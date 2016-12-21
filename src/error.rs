use region;
use mmap;

error_chain! {
    foreign_links {
        RegionFailure(region::error::Error);
        AllocateFailure(mmap::MapError);
    }

    errors {
        NotExecutable { display("address is not executable") }
        InvalidAddress { display("cannot read from address") }
        NoPatchArea { display("cannot find an inline patch area") }
        OutOfMemory { display("cannot allocate memory") }
        InvalidCode { display("function contains invalid assembly") }
        ExternalLoop { display("function contains an external loop") }
        UnsupportedRelativeBranch { display("function contains an unsupported branch") }
    }
}
