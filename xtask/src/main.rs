use std::path::Path;
use std::process::ExitCode;
use std::{env, fs, io};

use pump_testkit::Graph;

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("graphs") => match graphs() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("graphs: {error}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: cargo xtask graphs");
            ExitCode::FAILURE
        }
    }
}

fn graphs() -> io::Result<()> {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.docs");
    fs::create_dir_all(&out)?;
    let graph = Graph::standard();
    fs::write(out.join("therapy.mmd"), graph.to_mermaid())?;
    fs::write(
        out.join("therapy-overview.mmd"),
        graph.to_overview_mermaid(),
    )?;
    Ok(())
}
