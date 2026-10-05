use std::process::ExitCode;

pub fn run(json: bool) -> ExitCode {
    let config = match agentdust_core::paths::claude_config_dir() {
        Ok(config) => config,
        Err(_) => {
            eprintln!("agentdust disk: Claude configuration root unavailable");
            return ExitCode::FAILURE;
        }
    };
    let project = match std::env::current_dir() {
        Ok(project) => project,
        Err(_) => {
            eprintln!("agentdust disk: current project unavailable");
            return ExitCode::FAILURE;
        }
    };
    let report = agentdust_core::disk::report(&config, &project);
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(_) => return ExitCode::FAILURE,
        }
    } else {
        println!(
            "Claude Code disk report (read-only)\nBytes: logical / allocated. Allocated bytes are not reclaimable space."
        );
        for root in &report.roots {
            println!("\n{}: {}", root.root, root.status);
            for usage in &root.usage {
                let label = match usage.category {
                    agentdust_core::disk::Category::Rebuildable => "rebuildable",
                    agentdust_core::disk::Category::History => "history",
                    agentdust_core::disk::Category::Worktree => "worktree",
                    agentdust_core::disk::Category::ApplicationState => "application_state",
                    agentdust_core::disk::Category::Unknown => "unknown",
                };
                println!(
                    "  {label:18} {:>12} / {:>12} bytes  {} files, {} directories",
                    usage.logical_bytes, usage.allocated_bytes, usage.files, usage.directories
                );
            }
            let skipped = &root.skipped;
            println!(
                "  Skipped: {} symlinks, {} special files, {} other filesystems, {} duplicate inodes, {} errors, {} depth limits",
                skipped.symlinks,
                skipped.special_files,
                skipped.other_filesystems,
                skipped.duplicate_inodes,
                skipped.errors,
                skipped.depth_limit
            );
        }
        println!(
            "\n{} entries examined. Limit reached: {}. No files deleted.",
            report.entries_examined, report.limit_reached
        );
    }
    ExitCode::SUCCESS
}
