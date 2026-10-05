mod setup_support;

use setup_support::{Sandbox, code};

#[test]
fn apply_refuses_when_standard_streams_are_not_terminals() {
    let sandbox = Sandbox::new("apply-no-tty");
    let output = sandbox.run(&["apply"]);
    assert_eq!(code(&output), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires stdin and stdout attached to a terminal"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!sandbox.data.exists());
}
