use clap::Parser;

#[derive(Debug, Parser)]
#[command(version, about = "A plugin-powered coding agent")]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
