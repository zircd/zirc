//! A zirc server of your own, linked to zirc.dev: Ergo and its sidecar run together by one
//! program, which keeps the server's secrets and TLS certificate in its data directory, and
//! which zirc.dev manages over HTTPS once it has joined.
//! See README.md.

pub mod admin;
pub mod join;
pub mod media;
pub mod proxy;
pub mod run;
pub mod secrets;
pub mod store;
pub mod tls;
