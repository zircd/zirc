//! A throwaway Ergo for tests in other languages, such as the web client's Playwright tests:
//! prints its WebSocket address, then its plaintext IRC port, a line each. It then answers
//! each line on its standard input with one line:
//!
//! - `restart` restarts it, as [`Ergo::restart`] does, and it says `restarted` once it
//!   listens again.
//! - A JSON object with `web` starts a stand-in for zirc.dev that signs a person's app in
//!   to this server for real, answered with its address: see [`zirc_test_ergo::signin`].
//! - Any other is a JSON object asking for the IRC lines that say a zirc event, answered in
//!   JSON, an error too: see [`zirc_test_ergo::stage`]. A harness sends those lines over
//!   its own connection; it never writes zirc's framing itself.
//!
//! With `ZIRC_SERVE_CONTROL` set to a port, 0 for a free one, it also listens there for the
//! apps' end-to-end path, for a driver that can't hold its standard input (an iPhone's UI
//! test, a shell script): a third line says where, `control http://127.0.0.1:<port>`, and
//! [`zirc_test_ergo::control`] what it answers.
//!
//! It runs until its standard input closes or it's killed: a driver that only uses the
//! control port still holds the input open, so the server stops when the driver does,
//! however it ends. Finds Ergo as [`zirc_test_ergo::Ergo`] does.

use std::io::BufRead as _;
use std::sync::{Arc, Mutex, PoisonError};

use zirc_test_ergo::Ergo;
use zirc_test_ergo::scenario::Scenario;
use zirc_test_ergo::stage::Stager;
use zirc_test_ergo::{control, signin};

/// The variable naming the control port.
const CONTROL: &str = "ZIRC_SERVE_CONTROL";

fn main() {
    let Some(ergo) = Ergo::start() else {
        std::process::exit(2);
    };
    let (port, ws_port) = (ergo.port(), ergo.ws_port());
    // The server is the scenario's, which restarts it and brings its teammates back.
    let scenario = Arc::new(Mutex::new(Scenario::new(ergo)));
    println!("ws://127.0.0.1:{ws_port}");
    println!("{port}");
    if let Ok(asked) = std::env::var(CONTROL) {
        let started = asked
            .parse()
            .map_err(|_| format!("{CONTROL}={asked} isn't a port"))
            .and_then(|port| {
                control::start(Arc::clone(&scenario), port).map_err(|error| error.to_string())
            });
        match started {
            Ok(address) => println!("control {address}"),
            Err(error) => {
                eprintln!("serve: the control port didn't start: {error}");
                stop(&scenario);
                std::process::exit(2);
            }
        }
    }
    let mut stager = Stager::for_server(port);
    for line in std::io::stdin().lock().lines() {
        match line.as_deref() {
            Ok("restart") => {
                if let Err(error) = lock(&scenario).restart() {
                    eprintln!("serve: {error}");
                }
                println!("restarted");
            }
            Ok("") => {}
            Ok(ask) => match signin::answer(ask, port) {
                Some(started) => println!("{started}"),
                None => println!("{}", stager.answer(ask)),
            },
            Err(_) => break,
        }
    }
    // The control port's threads share the scenario, so it isn't dropped as this returns.
    stop(&scenario);
}

fn lock(scenario: &Mutex<Scenario>) -> std::sync::MutexGuard<'_, Scenario> {
    scenario.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Stops the server.
fn stop(scenario: &Mutex<Scenario>) {
    lock(scenario).stop();
}
