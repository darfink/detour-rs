//! Error types and utilities.
use region;
use mmap;

error_chain! {
    foreign_links {
        RegionFailure(region::error::Error);
        AllocateFailure(mmap::MapError);
    }

    errors {
        /// A static detour has already been initialized
        AlreadyExisting { display("detour has already been initialized") }
        /// The address does not contain valid instructions.
        InvalidCode { display("address contains invalid assembly") }
        /// The address has no available area for patching.
        NoPatchArea { display("cannot find an inline patch area") }
        /// The address is not executable memory.
        NotExecutable { display("address is not executable") }
        /// The system is out of executable memory.
        OutOfMemory { display("cannot allocate memory") }
        /// The address contains an external loop.
        UnsupportedLoop { display("address contains an unsupported loop") }
        /// The address contains an unsupported relative branch.
        UnsupportedRelativeBranch { display("address contains an unsupported branch") }
    }
}
