//! The asking half of searching a conversation ([`zirc_chat::chat::Search`]) for the apps
//! that sign in to zirc.dev with an account: the desktop, the phones and the TUI. The web
//! client asks with its own sign-in.

use zirc_chat::chat::Search;
use zirc_proto::login::Account;

use crate::login::{Client, Error, HistoryQuery};

/// How many matches a page asks for: the most the server gives.
pub const PAGE: usize = 200;

/// Asks zirc.dev, as `account`, for the next page of older matches `search` needs, and
/// adds them. Returns how many were added: none once it's all been found, or when zirc.dev
/// doesn't take the account (it's then done).
///
/// # Errors
///
/// When zirc.dev can't be reached or refuses, saying why, such as for a conversation the
/// organisation doesn't let its people read.
pub async fn more(search: &mut Search, client: &Client, account: &Account) -> Result<usize, Error> {
    let Some(asked) = search.server_query() else {
        return Ok(0);
    };
    let query = HistoryQuery {
        since: asked.since.to_rfc3339(),
        until: asked.until.map(|until| until.to_rfc3339()),
        conversation: Some(asked.conversation),
        sender: None,
        text: Some(asked.text),
        limit: PAGE,
        cursor: asked.cursor,
        events: false,
    };
    let before = search.hits.len();
    match client.history(account, &query).await? {
        Some(page) => search.take_page(&page),
        None => search.done = true,
    }
    Ok(search.hits.len() - before)
}
