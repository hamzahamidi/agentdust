use std::process::ExitCode;

fn main() -> ExitCode {
    agentdust_bench::cli::run(
        std::env::args_os()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
    )
}
