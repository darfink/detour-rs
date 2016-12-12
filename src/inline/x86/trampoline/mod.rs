use libc;
use inline::x86::udis;
use inline::pic;
use error::*;

mod gen;

// TODO: Document this sh111t
pub struct Trampoline {
    builder: pic::CodeBuilder,
    prolog_size: usize,
}

impl Trampoline {
    /// Constructs a new trampoline for the specified function.
    pub unsafe fn new(target: *const(), margin: usize) -> Result<Trampoline> {
        let (builder, prolog_size) = gen::Generator::process(udis_create(target), target, margin)?;
        Ok(Trampoline {
            prolog_size: prolog_size,
            builder: builder,
        })
    }

    /// Returns a reference to the trampoline's code generator.
    pub fn builder(&self) -> &pic::CodeBuilder {
        &self.builder
    }

    /// Returns the size of the prolog (i.e the amount of disassembled bytes).
    pub fn prolog_size(&self) -> usize {
        self.prolog_size
    }
}

/// Creates a default x86 disassembler
unsafe fn udis_create(target: *const ()) -> udis::ud {
    let mut ud = ::std::mem::zeroed();
    udis::ud_init(&mut ud);
    udis::ud_set_user_opaque_data(&mut ud, target as *mut _);
    udis::ud_set_input_hook(&mut ud, Some(udis_read_address));
    udis::ud_set_mode(&mut ud, (::std::mem::size_of::<usize>() * 8) as u8);
    ud
}

/// Reads one byte from a pointer an advances it.
unsafe extern "C" fn udis_read_address(ud: *mut udis::ud) -> libc::c_int {
    let pointer = udis::ud_get_user_opaque_data(ud) as *mut u8;
    let result = *pointer;
    udis::ud_set_user_opaque_data(ud, pointer.offset(1) as *mut _);
    result as _
}
