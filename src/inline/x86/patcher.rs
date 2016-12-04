use std::{mem, slice};
use region;

use super::thunk;
use inline::pic;
use error::*;
use util;

pub struct Patcher {
    patched: bool,
    patch_area: &'static mut [u8],
    detour_bounce: Vec<u8>,
    target_backup: Vec<u8>,
}

impl Patcher {
    // TODO: add relay function for x64
    // TODO: allocate memory close to target
    pub unsafe fn new(target: *const (), detour: *const (), prolog_size: usize) -> Result<Patcher> {
        // Ensure that the detour can be reached with a relative jump
        #[cfg(target_arch = "x86_64")]
        assert!((target as isize - detour as isize).abs() < i32::max_value() as isize);

        let jump_rel32_size = mem::size_of::<thunk::x86::JumpRel>();
        let jump_rel08_size = mem::size_of::<thunk::x86::JumpShort>();

        // Check if there isn't enough space for a relative long jump
        let patch_area = if !Self::is_patchable(target, prolog_size, jump_rel32_size) {
            // ... otherwise check if a relative small jump fits
            if Self::is_patchable(target, prolog_size, jump_rel08_size) {
                // A small jump relies on there being a hot patch area above the
                // function, that consists of at least 5 bytes (a rel32 jump).
                let hot_patch = (target as usize).wrapping_sub(jump_rel32_size);
                let hot_patch_area = slice::from_raw_parts(hot_patch as *const u8, jump_rel32_size);

                // Assert that the area only contains padding
                if !Self::is_code_padding(hot_patch_area) {
                    bail!(ErrorKind::NoPatchArea);
                }

                // Ensure that the hot patch area is executable
                if !util::is_executable_address(hot_patch_area.as_ptr() as *const _)? {
                    bail!(ErrorKind::NotExecutable);
                }

                // The range is from the start of the hot patch to the end of the jump
                let patch_size = jump_rel32_size + jump_rel08_size;
                slice::from_raw_parts_mut(hot_patch as *mut u8, patch_size)
            } else {
                bail!(ErrorKind::NoPatchArea);
            }
        } else {
            // The range is from the start of the function to the end of the jump
            slice::from_raw_parts_mut(target as *mut u8, jump_rel32_size)
        };

        let mut generator = pic::Generator::new();

        // Both hot patch and normal detours use a relative long jump
        generator.add_thunk(thunk::x86::jmp_rel32(detour as u32));

        // The hot patch relies on a small jump to land on the long jump
        if patch_area.len() > jump_rel32_size {
            let displacement = -(jump_rel32_size as i8);
            generator.add_thunk(thunk::x86::jmp_rel8(displacement));
        }

        let backup = patch_area.to_vec();
        let patch_address = patch_area.as_ptr() as *const ();

        Ok(Patcher {
            patched: false,
            patch_area: patch_area,
            target_backup: backup,
            detour_bounce: generator.generate(patch_address),
        })
    }

    /// Either patches or removes a patch from a function.
    pub unsafe fn toggle(&mut self, enable: bool) -> Result<()> {
        if self.patched == enable {
            return Ok(());
        }

        // Runtime code is by default only read-execute
        let mut region = region::View::new(self.patch_area.as_ptr(), self.patch_area.len())?;

        region.exec_with_prot(region::Protection::ReadWriteExecute, || {
            // Copy either the detour or the original bytes of the function
            self.patch_area.copy_from_slice(if enable {
                &self.detour_bounce
            } else {
                &self.target_backup
            });
        })?;

        self.patched = enable;
        Ok(())
    }

    /// Returns whether the function is patched or not.
    pub fn is_patched(&self) -> bool {
        self.patched
    }

    /// Returns the default size of a patch.
    pub fn patch_size(_target: *const ()) -> usize {
        mem::size_of::<thunk::x86::JumpRel>()
    }

    /// Returns whether an address can be inline patched or not.
    unsafe fn is_patchable(target: *const(), prolog_size: usize, patch_size: usize) -> bool {
        if prolog_size >= patch_size {
            // If the whole patch fits it's good to go!
            true
        } else {
            // Otherwise the inline patch relies on padding after the prolog
            let slice = slice::from_raw_parts(
                (target as usize + prolog_size) as *const u8,
                patch_size - prolog_size);
            Self::is_code_padding(slice)
        }
    }

    /// Returns true if the slice only contains code padding.
    fn is_code_padding(buffer: &[u8]) -> bool {
        const PADDING: [u8; 3] = [0x00, 0x90, 0xCC];
        buffer.iter().all(|code| PADDING.contains(code))
    }
}
