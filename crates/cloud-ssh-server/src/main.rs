//! Cloud SSH server entry point.

use clap::{Parser, Subcommand};
use cloud_ssh_core::{ResizePolicy, TerminalSize};

/// Cloud SSH command-line interface.
#[derive(Debug, Parser)]
#[command(name = "cloud-ssh")]
#[command(about = "Collaborative terminal room runtime")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print the current development server plan.
    Plan,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Plan) {
        Command::Plan => print_plan(),
    }
}

fn print_plan() {
    let default_size = TerminalSize::new(40, 120);
    let resize_policy = ResizePolicy::Controller;

    println!("Cloud SSH development server placeholder");
    println!("default_size={}x{}", default_size.cols, default_size.rows);
    println!("resize_policy={resize_policy:?}");
}
