mod hook;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["hook", "claude"] => {
            hook::run_claude();
            ExitCode::SUCCESS
        }
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: agentdust hook claude | agentdust version");
            ExitCode::from(2)
        }
    }
}
