use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    tester::main(
        std::env::args().skip(1),
        &mut io::stdout(),
        &mut io::stderr(),
    )
}
