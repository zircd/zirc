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

/// A direct message the server refused because whom it's to isn't connected, as a machine
/// whose zircd is stopped: the send fails where the person is looking, naming the machine,
/// and what was written goes back in the composer, with nothing sent again by itself (the
/// user's choice, 25 Nov 2026). One delivered meanwhile, or one to another nick, stays sent.
#[test]
fn a_message_to_a_machine_not_connected_goes_back_to_the_composer() {
    let mut chat = chat();
    connected(&mut chat);
    let agent = "mattias-mbp/cc-1531f8";
    let at = chat.ensure(agent, Kind::Query);
    let _ = chat.select(at);
    // The field still says it as it's sent, as an app empties it only after.
    chat.set_draft("rebase on main please");
    let sent = chat.say(agent, "rebase on main please", false);
    assert_eq!(
        sent.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["PRIVMSG mattias-mbp :cc-1531f8 rebase on main please"]
    );
    let _ = chat.say("ada", "lunch?", false);
    // Ada had hers; the machine wasn't there.
    let resent = feed(
        &mut chat,
        &[
            ":me!u@h PRIVMSG ada :lunch?",
            ":srv 401 me mattias-mbp :No such nick",
        ],
    );
    assert!(
        resent.is_empty(),
        "nothing sent again by itself: {resent:?}"
    );
    // An app emptying its field for the send after the refusal came doesn't undo it.
    chat.set_draft("");
    assert_eq!(chat.draft_in(agent), "rebase on main please");
    assert_eq!(chat.draft_restored_in(agent), 1);
    assert_eq!(chat.draft_restored_in("ada"), 0);
    let said = &chat.selected().entries.last().unwrap().kind;
    assert_eq!(
        said,
        &EntryKind::Info(not_connected("mattias-mbp")),
        "said where the person is looking"
    );

    // Something already written after it is kept, below it; a 401 of nothing of ours is an
    // error as before, and gives nothing back.
    let _ = chat.say(agent, "and the tests", false);
    chat.set_draft("meanwhile");
    feed(&mut chat, &[":srv 401 me mattias-mbp :No such nick"]);
    assert_eq!(chat.draft_in(agent), "and the tests\nmeanwhile");
    feed(&mut chat, &[":srv 401 me mattias-mbp :No such nick"]);
    assert_eq!(chat.draft_restored_in(agent), 2);
}
