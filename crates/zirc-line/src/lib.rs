//! IRC's line format, without any I/O: [`Line`] parses and writes IRC lines,
//! [`multiline`] makes and reassembles `draft/multiline` batches, and [`registration`]
//! registers with a server. `zirc-irc` connects with them; the chat model in `zirc-chat`
//! reads and writes them, in the browser too.

mod line;
pub mod multiline;
pub mod registration;

pub use line::{Line, ParseError};
