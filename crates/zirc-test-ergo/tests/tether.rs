//! A test's Ergo ends with the process that started it, however that ends
//! (`zirc_test_ergo::tether`): here `serve`, killed as Bazel's timeout kills a test (SIGKILL)
//! and as the Android app's end-to-end run stopped it after every run (SIGTERM), whose Ergo
//! then ran on for days.

use std::io::{BufRead as _, BufReader};
use std::net::{Ipv4Addr, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Starts `serve`, sends it `signal`, and says whether its Ergo stopped listening.
fn ergo_ends_with_serve(signal: &str) {
    let Some(serve) = std::env::var_os("ZIRC_SERVE") else {
        eprintln!("skipped: ZIRC_SERVE isn't set, as Bazel sets it");
        return;
    };
    let mut serve = Command::new(serve)
        // Held open, so `serve` runs until it's killed.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("starting serve");
    let mut said = BufReader::new(serve.stdout.take().unwrap()).lines();
    // Its WebSocket address, then its IRC port.
    said.next();
    let port: u16 = said.next().unwrap().unwrap().parse().unwrap();
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok());

    // The shell's own kill, as CI's image has no kill command.
    let killed = Command::new("sh")
        .args(["-c", &format!("kill -{signal} {}", serve.id())])
        .status()
        .unwrap();
    assert!(killed.success());
    serve.wait().unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "Ergo still listens on {port} after serve got SIG{signal}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn ergo_ends_when_serve_is_killed() {
    ergo_ends_with_serve("KILL");
}

#[test]
fn ergo_ends_when_serve_is_terminated() {
    ergo_ends_with_serve("TERM");
}
