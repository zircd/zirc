//! What's written in each conversation's composer, kept by the core (chat/drafts.rs).

use super::*;

#[test]
fn a_draft_stays_with_its_conversation_and_never_follows_into_another() {
    let mut chat = chat();
    connected(&mut chat);
    feed(&mut chat, &[":me!u@h JOIN #web"]);
    let api = chat.position("#api").unwrap();
    let web = chat.position("#web").unwrap();
    let _ = chat.select(api);
    chat.set_draft("half a thought");

    let _ = chat.select(web);
    assert_eq!(chat.draft(), "");
    chat.set_draft("over here");
    let _ = chat.select(api);
    assert_eq!(chat.draft(), "half a thought");

    // Sent, it's gone; the other's stays.
    chat.set_draft("");
    let _ = chat.select(web);
    assert_eq!(chat.draft(), "over here");
}

#[test]
fn a_draft_is_kept_by_its_conversations_name_too() {
    let mut chat = chat();
    connected(&mut chat);
    feed(&mut chat, &[":me!u@h JOIN #web"]);
    chat.set_draft_in("#web", "for later");
    chat.set_draft_in("#nowhere", "lost");
    assert_eq!(chat.draft_in("#WEB"), "for later");
    assert_eq!(chat.draft_in("#nowhere"), "");
    let _ = chat.select(chat.position("#web").unwrap());
    assert_eq!(chat.draft(), "for later");
}
