use std::io::{self, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|arg| !["--ipv6", "--ignore-term"].contains(&arg.as_str()))
    {
        return Err(io::Error::other("invalid fixture arguments"));
    }
    if args.iter().any(|arg| arg == "--ignore-term") {
        // SAFETY: SIG_IGN is a valid disposition and no handler code is installed.
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    let address = if args.iter().any(|arg| arg == "--ipv6") {
        "[::1]:0"
    } else {
        "127.0.0.1:0"
    };
    let listener = TcpListener::bind(address)?;
    println!("{}", listener.local_addr()?.port());
    io::stdout().flush()?;
    thread::sleep(Duration::from_secs(120));
    drop(listener);
    Ok(())
}
