//! The scene a chat test starts from, so the test itself says only what is different: a
//! chat that is connected and in `#api`, with the people the test needs. A [`Scene`] is a
//! [`Chat`], so what a test asserts it asks of the chat directly.
//!
//! ```ignore
//! let mut scene = Scene::api_with_away();   // environment: alice and carol away, bob here
//! scene.hears(&[":bob!u@h AWAY :lunch"]);   // action
//! assert_eq!(scene.away("bob"), Some("lunch")); // assertion
//! ```

use std::ops::{Deref, DerefMut};

use super::*;

/// A chat, and what it takes to put it where a test starts.
pub struct Scene {
    chat: Chat,
}

impl Deref for Scene {
    type Target = Chat;
    fn deref(&self) -> &Chat {
        &self.chat
    }
}

impl DerefMut for Scene {
    fn deref_mut(&mut self) -> &mut Chat {
        &mut self.chat
    }
}

impl Scene {
    /// A chat as `me` that is connected to the server and has joined `#api`.
    pub fn in_api() -> Self {
        let mut chat = super::chat();
        connected(&mut chat);
        Self { chat }
    }

    /// `#api` with alice and carol away and bob here, as WHO says at joining. WHOX's flags
    /// may say more than `H` or `G`.
    pub fn api_with_away() -> Self {
        let mut scene = Self::in_api();
        scene.hears(&[
            ":s 353 me = #api :me alice bob carol",
            ":s 366 me #api :End",
            ":s 354 me 42 h alice G alice :Alice",
            ":s 354 me 42 h bob H@ bob :Bob",
            ":s 352 me #api u h s carol G :0 Carol",
        ]);
        scene
    }

    /// The server sends `lines`, and the chat's replies to them are of no interest.
    pub fn into_chat(self) -> Chat {
        self.chat
    }

    pub fn hears(&mut self, lines: &[&str]) -> &mut Self {
        feed(&mut self.chat, lines);
        self
    }

    /// Who is in `#api` and what each one's away message is, by nick.
    pub fn away_in_api(&self) -> Vec<(String, Option<String>)> {
        self.chat
            .people("#api")
            .into_iter()
            .map(|person| (person.nick, person.away))
            .collect()
    }

    /// What the selected conversation shows, as lines of text.
    pub fn shown(&self) -> Vec<String> {
        said(self.chat.selected())
    }
}
