//! zirc's shared logic, used by the daemon, the CLI and the apps alike.

pub mod account;
pub mod account_keys;
pub mod account_text;
pub mod admin;
pub mod agents;
pub mod cards;
pub mod channels;
pub mod clashes;
pub mod connection;
pub mod decisions;
pub mod device_key;
pub mod editor;
/// Emoji, for pickers and the composer: zirc-chat's, which the browser shares.
pub use zirc_chat::emoji;
pub mod filter;
pub mod fit;
pub mod generated;
pub mod gotchas;
pub mod guest;
pub mod hints;
pub mod history;
pub mod hunks;
pub mod intent_scope;
pub mod ipc;
pub mod leases;
pub mod login;
pub mod machine;
pub mod media;
pub mod notes;
pub mod one_line;
pub mod owners;
pub mod paths;
pub mod playbook;
pub mod project;
pub mod reports;
pub mod same_work;
pub mod sealed;
pub mod search;
pub mod secrets;
pub mod send;
pub mod sessions;
pub mod settings;
pub mod siblings;
pub mod signin;
pub mod signout;
pub mod stacked;
pub mod states;
pub mod team_hints;
pub mod telemetry;
pub mod time;
pub mod undone;
pub mod update;
pub mod waits;
pub mod which_org;

pub use zirc_chat::{
    chat, clock, command, compose, keys, link_ask, names, orgs, presence, trim, utf16,
};
