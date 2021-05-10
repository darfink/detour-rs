use super::thunk;
use crate::error::{Error, Result};
use crate::{pic, util};
use std::{mem, slice};

pub struct Patcher {
  patch_area: &'static mut [u8],
  original_prolog: Vec<u8>,
  detour_prolog: Vec<u8>,
}

impl Patcher {
  /// Creates a new detour patcher for an address.
  ///
  /// # Arguments
  ///
  /// * `target` - An address that should be hooked.
  /// * `detour` - An address that the target should be redirected to.
  /// * `prolog_size` - The available inline space for the hook.
  pub unsafe fn new(target: *const (), detour: *const (), prolog_size: usize) -> Result<Patcher> {
    // Calculate the patch area (i.e if a short or long jump should be used)
    let patch_area = Self::patch_area(target, prolog_size)?;
    let emitter = Self::hook_template(detour, patch_area);

    let patch_address = patch_area.as_ptr() as *const ();
    let original_prolog = patch_area.to_vec();

    Ok(Patcher {
      detour_prolog: emitter.emit(patch_address),
      original_prolog,
      patch_area,
    })
  }

  /// Returns the target's patch area.
  pub fn area(&self) -> &[u8] {
    self.patch_area
  }

  /// Either patches or unpatches the function.
  pub unsafe fn toggle(&mut self, enable: bool) {
    // Copy either the detour or the original bytes of the function
    self.patch_area.copy_from_slice(if enable {
      &self.detour_prolog
    } else {
      &self.original_prolog
    });
  }

  /// Returns the patch area for a function, consisting of a long jump and
  /// possibly a short jump.
  unsafe fn patch_area(target: *const (), prolog_size: usize) -> Result<&'static mut [u8]> {
    let jump_rel08_size = mem::size_of::<thunk::x86::JumpShort>();
    let jump_rel32_size = mem::size_of::<thunk::x86::JumpRel>();

    // Check if there isn't enough space for a relative long jump
    if !Self::is_patchable(target, prolog_size, jump_rel32_size) {
      // ... check if a relative small jump fits instead
      if Self::is_patchable(target, prolog_size, jump_rel08_size) {
        // A small jump relies on there being a hot patch area above the
        // function, that consists of at least 5 bytes (a rel32 jump).
        let hot_patch = target as usize - jump_rel32_size;
        let hot_patch_area = slice::from_raw_parts(hot_patch as *const u8, jump_rel32_size);

        // Ensure that the hot patch area only contains padding and is executable
        if !Self::is_code_padding(hot_patch_area)
          || !util::is_executable_address(hot_patch_area.as_ptr() as *const _)?
        {
          Err(Error::NoPatchArea)?;
        }

        // The range is from the start of the hot patch to the end of the jump
        let patch_size = jump_rel32_size + jump_rel08_size;
        Ok(slice::from_raw_parts_mut(hot_patch as *mut u8, patch_size))
      } else {
        Err(Error::NoPatchArea)
      }
    } else {
      // The range is from the start of the function to the end of the jump
      Ok(slice::from_raw_parts_mut(
        target as *mut u8,
        jump_rel32_size,
      ))
    }
  }

  /// Creates a redirect code template for the targetted patch area.
  fn hook_template(detour: *const (), patch_area: &[u8]) -> pic::CodeEmitter {
    let mut emitter = pic::CodeEmitter::new();

    // Both hot patch and normal detours use a relative long jump
    emitter.add_thunk(thunk::x86::jmp_rel32(detour as usize));

    // The hot patch relies on a small jump to get to the long jump
    let jump_rel32_size = mem::size_of::<thunk::x86::JumpRel>();
    let uses_hot_patch = patch_area.len() > jump_rel32_size;

    if uses_hot_patch {
      let displacement = -(jump_rel32_size as i8);
      emitter.add_thunk(thunk::x86::jmp_rel8(displacement));
    }

    // Pad leftover bytes with nops
    while emitter.len() < patch_area.len() {
      emitter.add_thunk(thunk::x86::nop());
    }

    emitter
  }

  /// Returns whether an address can be inline patched or not.
  unsafe fn is_patchable(target: *const (), prolog_size: usize, patch_size: usize) -> bool {
    if prolog_size >= patch_size {
      // If the whole patch fits it's good to go!
      return true;
    }

    // Otherwise the inline patch relies on padding after the prolog
    let slice = slice::from_raw_parts(
      (target as usize + prolog_size) as *const u8,
      patch_size - prolog_size,
    );

    Self::is_code_padding(slice)
  }

  /// Returns true if the slice only contains code padding.
  fn is_code_padding(buffer: &[u8]) -> bool {
    const PADDING: [u8; 3] = [0x00, 0x90, 0xCC];
    buffer.iter().all(|code| PADDING.contains(code))
  }
}
