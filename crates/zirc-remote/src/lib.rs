//! A host that agents control by direct message: it logs in as a machine's account, hears
//! the queries (`zirc ask`) sent to it, runs the commands its JSON config declares for those
//! who may call them, and answers each with the output: an agent's query with an answer event,
//! which zirc shows as information, and the owner's own plain message with a plain reply, so
//! the owner can chat with it from any app.
//!
//! It carries nothing else of zirc: no sessions, no watch, no channels.

pub mod config;
mod serve;

pub use config::Config;
pub use serve::{Options, run};
