use std::process::ExitCode;

pub fn run(args: &[&str]) -> ExitCode {
    let Some(port) = args
        .first()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
    else {
        eprintln!("usage: agentdust port PORT [--resolve] [--json]");
        return ExitCode::from(2);
    };
    let flags = &args[1..];
    if flags.iter().any(|flag| !["--resolve", "--json"].contains(flag))
        || flags.len() > 2
        || (flags.len() == 2 && flags[0] == flags[1])
    {
        eprintln!("usage: agentdust port PORT [--resolve] [--json]");
        return ExitCode::from(2);
    }
    let result = agentdust_core::paths::data_dir()
        .and_then(|dir| agentdust_core::port_live::run(&dir, port, flags.contains(&"--resolve")));
    match result {
        Ok(report) => {
            if flags.contains(&"--json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("port report serializes")
                );
            } else {
                println!(
                    "TCP port {}: {} (current-user visibility)",
                    report.port, report.state
                );
                for listener in &report.listeners {
                    println!(
                        "PID {}: {}{}",
                        listener.pid,
                        listener.result,
                        listener
                            .reason
                            .as_ref()
                            .map(|reason| format!(" ({reason})"))
                            .unwrap_or_default()
                    );
                    if let Some(finding) = &listener.finding {
                        println!("  {}", agentdust_core::finding::HumanDisplay::prompt(finding));
                    }
                }
                if let Some(error) = report.error {
                    println!("Evidence unavailable: {error}");
                }
                println!(
                    "No visible listener is an observation, not a guarantee that a bind will succeed. Uncertain actionable processes require the plan/apply approval flow."
                );
            }
            if report.error.is_some() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("agentdust port: {error}");
            ExitCode::FAILURE
        }
    }
}
