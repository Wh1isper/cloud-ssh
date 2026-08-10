use std::{env, process::ExitCode};

const FOUNDATION_MESSAGE: &str =
    "OwlMux Relay foundation: enrollment and reverse transport are not implemented yet.";

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("owlmux-relay {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("owlmux-relay {}", env!("CARGO_PKG_VERSION"));
            println!();
            println!("{FOUNDATION_MESSAGE}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("{FOUNDATION_MESSAGE}");
            ExitCode::FAILURE
        }
        Some(argument) => {
            eprintln!("unknown argument: {argument}");
            eprintln!("{FOUNDATION_MESSAGE}");
            ExitCode::FAILURE
        }
    }
}
