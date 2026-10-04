mod hook;

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "hook") {
        hook::run();
        return ExitCode::SUCCESS;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["mcp"] => mcp(),
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: agentdust hook claude | agentdust mcp | agentdust version");
            ExitCode::from(2)
        }
    }
}

fn mcp() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("agentdust mcp: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(agentdust_mcp::serve_stdio()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("agentdust mcp: {err}");
            ExitCode::FAILURE
        }
    }
}
