//! zirc's chat model, without any I/O: what a chat session holds and shows ([`chat`]), a
//! chat per organisation ([`orgs`]), who's working on what ([`presence`]), when a message
//! names someone ([`names`]), what a typed `/command` asks for ([`command`]), taking
//! messages back out of history ([`trim`]), keeping the connection up ([`connection`]), and
//! what time it is ([`clock`]).
//! It takes lines from the server and answers the lines to send, so it runs wherever the
//! connection is made: in `zirc-core`'s apps, and in the browser.

pub mod chat;
pub mod clock;
pub mod command;
pub mod connection;
pub mod emoji;
pub mod link_ask;
pub mod names;
pub mod orgs;
pub mod presence;
pub mod trim;
pub mod utf16;
