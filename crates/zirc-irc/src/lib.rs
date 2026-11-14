//! A small IRC client: [`connect`] registers with a server and hands back a [`Client`] to
//! send with and a channel of what arrives. [`Line`] and [`multiline`] are `zirc-line`'s,
//! which has no I/O.

mod client;
mod websocket;

pub use client::{Bearer, Client, Config, Credentials, Error, Incoming, connect};
pub use zirc_line::{Line, ParseError, multiline};
