use std::process::Command;

#[test]
fn version_prints_the_crate_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .arg("version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("agentdust {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn an_unknown_command_exits_with_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentdust"))
        .arg("nope")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("usage: agentdust")
    );
}
