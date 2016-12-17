use std::{mem, slice};
use region;

use {util, pic};
use error::*;
use super::thunk;

pub struct Patcher {
    patched: bool,
    patch_area: &'static mut [u8],
    detour_bounce: Vec<u8>,
    target_backup: Vec<u8>,
}

impl Patcher {
    /// Creates a new detour patcher for a specific function.
    pub unsafe fn new(target: *const (), detour: *const (), prolog_size: usize) -> Result<Patcher> {
        // Ensure that the detour can be reached with a relative jump (+/- 2GB)
        #[cfg(target_arch = "x86_64")]
        assert!((target as isize).wrapping_sub(detour as isize).abs() < i32::max_value() as isize);

        // Calculate the patch area (i.e if a short or long jump should be used)
        let patch_area = Self::get_patch_area(target, prolog_size)?;
        let hook = Self::hook_builder(detour, patch_area);

        let patch_address = patch_area.as_ptr() as *const ();
        let patch_code = patch_area.to_vec();

        Ok(Patcher {
            patched: false,
            patch_area: patch_area,
            target_backup: patch_code,
            detour_bounce: hook.build(patch_address),
        })
    }

    /// Either patches or unpatches a function.
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
    pub fn default_patch_size(_target: *const ()) -> usize {
        mem::size_of::<thunk::x86::JumpRel>()
    }

    /// Returns the patch area for a function, consisting of a long jump and possibly a short jump.
    unsafe fn get_patch_area(target: *const (), prolog_size: usize) -> Result<&'static mut [u8]> {
        let jump_rel32_size = mem::size_of::<thunk::x86::JumpRel>();
        let jump_rel08_size = mem::size_of::<thunk::x86::JumpShort>();

        // Check if there isn't enough space for a relative long jump
        if !Self::is_patchable(target, prolog_size, jump_rel32_size) {
            // ... check if a relative small jump fits instead
            if Self::is_patchable(target, prolog_size, jump_rel08_size) {
                // A small jump relies on there being a hot patch area above the
                // function, that consists of at least 5 bytes (a rel32 jump).
                let hot_patch = target as usize - jump_rel32_size;
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
                Ok(slice::from_raw_parts_mut(hot_patch as *mut u8, patch_size))
            } else {
                bail!(ErrorKind::NoPatchArea);
            }
        } else {
            // The range is from the start of the function to the end of the jump
            Ok(slice::from_raw_parts_mut(target as *mut u8, jump_rel32_size))
        }
    }

    /// Creates template code for the targetted patch area.
    fn hook_builder(detour: *const (), patch_area: &[u8]) -> pic::CodeBuilder {
        let mut builder = pic::CodeBuilder::new();

        // Both hot patch and normal detours use a relative long jump
        builder.add_thunk(thunk::x86::jmp_rel32(detour as u32));

        // The hot patch relies on a small jump to get to the long jump
        let jump_rel32_size = mem::size_of::<thunk::x86::JumpRel>();
        let uses_hot_patch = patch_area.len() > jump_rel32_size;

        if uses_hot_patch {
            let displacement = -(jump_rel32_size as i8);
            builder.add_thunk(thunk::x86::jmp_rel8(displacement));
        }

        builder
    }

    /// Returns whether an address can be inline patched or not.
    unsafe fn is_patchable(target: *const (), prolog_size: usize, patch_size: usize) -> bool {
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
