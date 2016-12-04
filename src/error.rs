use region;
use std;

error_chain! {
    foreign_links {
        RegionFailure(region::error::Error);
        AllocateFailure(std::io::Error);
    }

    errors {
        NotExecutable { display("address is not executable") }
        IsExecutable { display("address is executable") }
        InvalidAddress { display("cannot read from address") }
        InvalidCode { display("function contains invalid code") }
        ExternalLoop { display("function contains a loop with external destination") }
        NoPatchArea { display("cannot find an inline patch area") }
        UnsupportedRelativeBranch  {
            display("function contains unhandled relative branching")
        }
    }
}
