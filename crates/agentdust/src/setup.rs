use std::fs;
use std::io::{self, IsTerminal};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agentdust_agents::claude_setup::{self, InstallOutcome, RemoveOutcome, SetupEnv};
use agentdust_agents::hook_config::stable_exe;
use agentdust_agents::native_cli::{CliRunner, SystemRunner};
use agentdust_core::paths;

const USAGE: &str = "usage: agentdust setup [codex] [--check | --remove] [--yes]";
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Install,
    Check,
    Remove,
}

struct Options {
    mode: Mode,
    yes: bool,
    codex: bool,
}

fn parse(args: &[&str]) -> Option<Options> {
    let mut mode = None;
    let mut yes = false;
    let mut codex = false;
    for arg in args {
        match *arg {
            "--check" | "--remove" => {
                let chosen = if *arg == "--check" {
                    Mode::Check
                } else {
                    Mode::Remove
                };
                if mode.replace(chosen).is_some() {
                    return None;
                }
            }
            "--yes" if !yes => yes = true,
            "codex" if !codex => codex = true,
            _ => return None,
        }
    }
    let mode = mode.unwrap_or(Mode::Install);
    if mode == Mode::Check && yes {
        return None;
    }
    Some(Options { mode, yes, codex })
}

pub struct Context {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub exe: PathBuf,
    claude_cli: Option<PathBuf>,
    runner: Option<SystemRunner>,
}

impl Context {
    pub fn detect() -> Result<Self, String> {
        let config_dir = paths::claude_config_dir()
            .map_err(|err| format!("cannot locate the Claude Code config directory: {err}"))?;
        let data_dir = paths::data_dir().map_err(|err| format!("cannot locate the data directory: {err}"))?;
        let exe = std::env::current_exe()
            .and_then(std::path::absolute)
            .map(|path| stable_exe(&path))
            .map_err(|err| format!("cannot locate the agentdust binary: {err}"))?;
        let claude_cli = find_on_path("claude");
        let runner = claude_cli
            .as_ref()
            .map(|cli| SystemRunner::new(cli.clone(), None, CLI_TIMEOUT));
        Ok(Self {
            config_dir,
            data_dir,
            exe,
            claude_cli,
            runner,
        })
    }

    pub fn env(&self) -> SetupEnv<'_> {
        SetupEnv {
            config_dir: self.config_dir.clone(),
            data_dir: self.data_dir.clone(),
            exe: self.exe.clone(),
            claude_cli: self.claude_cli.clone(),
            runner: self.runner.as_ref().map(|runner| runner as &dyn CliRunner),
        }
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

pub fn run(args: &[&str]) -> ExitCode {
    let Some(options) = parse(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if options.mode != Mode::Check && !options.yes && !interactive {
        eprintln!(
            "agentdust setup needs a terminal to ask for your consent. Run it in a terminal, or pass --yes to approve the printed changes without being asked."
        );
        return ExitCode::FAILURE;
    }
    if options.codex {
        return run_codex(options);
    }
    let context = match Context::detect() {
        Ok(context) => context,
        Err(err) => {
            eprintln!("agentdust setup: {err}");
            return ExitCode::FAILURE;
        }
    };
    let env = context.env();
    let mut ask = |plan: &str| -> bool {
        println!("{plan}");
        if options.yes {
            return true;
        }
        let mut stdout = io::stdout();
        claude_setup::ask_yes_no(
            &mut io::stdin().lock(),
            &mut stdout,
            "Apply these changes? [y/N] ",
        )
        .unwrap_or(false)
    };
    match options.mode {
        Mode::Check => {
            let report = claude_setup::check(&env, true);
            print!("{}", report.render());
            exit(report.ok())
        }
        Mode::Install => match claude_setup::install(&env, &mut ask) {
            Ok(InstallOutcome::Nothing { text, complete }) => {
                print!("{text}");
                exit(complete)
            }
            Ok(InstallOutcome::Declined) => declined(),
            Ok(InstallOutcome::Applied(report)) => {
                if report.succeeded() {
                    print!("{}", report.render());
                } else {
                    eprint!("{}", report.render());
                }
                exit(report.succeeded())
            }
            Err(err) => failed(&err.to_string()),
        },
        Mode::Remove => match claude_setup::remove(&env, &mut ask) {
            Ok(RemoveOutcome::Nothing { text }) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Ok(RemoveOutcome::Declined) => declined(),
            Ok(RemoveOutcome::Applied(report)) => {
                if report.complete {
                    print!("{}", report.text);
                } else {
                    eprint!("{}", report.text);
                }
                exit(report.complete)
            }
            Err(err) => failed(&err.to_string()),
        },
    }
}

fn exit(success: bool) -> ExitCode {
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn declined() -> ExitCode {
    println!("Cancelled. Nothing was changed.");
    ExitCode::FAILURE
}

fn failed(reason: &str) -> ExitCode {
    eprintln!("agentdust setup: {reason}");
    ExitCode::FAILURE
}

fn run_codex(options: Options) -> ExitCode {
    use agentdust_agents::codex_setup::{self, Mode as CodexMode};
    let result = (|| -> Result<(String, bool), String> {
        let config_dir = match std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".codex"),
        };
        if !config_dir.is_absolute() {
            return Err("CODEX_HOME must be absolute".into());
        }
        let cli = find_on_path("codex").ok_or("Codex CLI was not found on PATH")?;
        let runner = SystemRunner::for_codex(cli, config_dir.clone(), CLI_TIMEOUT);
        let exe = stable_exe(&std::env::current_exe().map_err(|e| e.to_string())?);
        let env = codex_setup::SetupEnv {
            config_dir,
            data_dir: paths::data_dir().map_err(|e| e.to_string())?,
            exe,
            runner: &runner,
        };
        let mode = match options.mode {
            Mode::Install => CodexMode::Install,
            Mode::Check => CodexMode::Check,
            Mode::Remove => CodexMode::Remove,
        };
        codex_setup::run(&env, mode, &mut |plan| {
            println!("{plan}");
            options.yes
                || claude_setup::ask_yes_no(
                    &mut io::stdin().lock(),
                    &mut io::stdout(),
                    "Apply these changes? [y/N] ",
                )
                .unwrap_or(false)
        })
    })();
    match result {
        Ok((text, complete)) => {
            print!("{text}");
            exit(complete)
        }
        Err(reason) => failed(&reason),
    }
}
