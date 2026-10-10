mod apply;
mod automatic;
#[cfg(not(target_os = "linux"))]
mod disk;
mod doctor;
mod hook;
mod metrics;
mod port;
mod setup;
#[cfg(not(target_os = "linux"))]
mod status;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "hook") {
        hook::run();
        return ExitCode::SUCCESS;
    }
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["setup", rest @ ..] => setup::run(rest),
        ["apply"] => apply::run(),
        ["auto", rest @ ..] => automatic::run(rest),
        ["port", rest @ ..] => port::run(rest),
        ["metrics"] => metrics::run(false),
        ["metrics", "--json"] => metrics::run(true),
        ["mcp"] => mcp(),
        ["doctor"] => doctor::run(false),
        ["doctor", "--json"] => doctor::run(true),
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "Linux enables diagnosis, Claude Code and Codex setup, MCP tools, foreground apply, opt-in automatic cleanup, metrics, and current-user TCP port diagnosis on writable ext4. Disk reports are unavailable."
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "hook") {
        hook::run();
        return ExitCode::SUCCESS;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["mcp"] => mcp(),
        ["port", rest @ ..] => port::run(rest),
        ["disk"] => disk::run(false),
        ["disk", "--json"] => disk::run(true),
        ["doctor"] => doctor::run(false),
        ["doctor", "--json"] => doctor::run(true),
        ["metrics"] => metrics::run(false),
        ["metrics", "--json"] => metrics::run(true),
        ["apply"] => apply::run(),
        ["auto", rest @ ..] => automatic::run(rest),
        ["setup", rest @ ..] => setup::run(rest),
        ["status"] => status::run(),
        ["status", ..] => {
            eprintln!("usage: agentdust status");
            ExitCode::from(2)
        }
        ["version"] => {
            println!("agentdust {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: agentdust hook claude | agentdust hook codex | agentdust mcp | agentdust doctor [--json] | agentdust metrics [--json] | agentdust disk [--json] | agentdust port PORT [--resolve] [--json] | agentdust apply | agentdust auto enable PROJECT | disable [PROJECT] | pause | resume | keep PID | unkeep PID | status | agentdust setup [codex] [--check | --remove] [--yes] | agentdust status | agentdust version"
            );
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
