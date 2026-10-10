#![cfg(target_os = "linux")]

use std::net::TcpListener;

use agentdust_core::linux_port_live::LinuxListeners;
use agentdust_core::port::ListenerSource;

#[test]
fn procfs_listener_inventory_returns_the_exact_current_user_process() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let found = LinuxListeners.listeners(port).unwrap();
    assert!(
        found
            .iter()
            .any(|identity| identity.kernel.pid == std::process::id() as i32),
        "listener PID was not present: {found:?}"
    );
}
