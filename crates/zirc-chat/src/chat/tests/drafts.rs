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

#[test]
fn enter_in_jump_to_opens_its_first_row() {
    let mut chat = chat();
    connected(&mut chat);
    feed(
        &mut chat,
        &[":srv 322 me #design 3 :Design", ":srv 323 me :End of /LIST"],
    );
    assert_eq!(chat.jump_first("ap").as_deref(), Some("#api"));
    assert_eq!(chat.jump_first("desi").as_deref(), Some("#design"));
    assert_eq!(
        chat.jump_first("launch"),
        chat.join_offer("launch").map(|offer| offer.name)
    );
    assert_eq!(chat.jump_first("  "), None);
}
