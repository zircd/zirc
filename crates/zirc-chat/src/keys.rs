//! What the keyboard does in the composer and around it, decided once for every app: what
//! Enter does, and which keys typed elsewhere go into the composer. Each app reads its key
//! event into [`KeyPressed`] and does what it's told, so no app decides it again.

/// A key press as an app reads it: which modifiers were held, and where it came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a key event's own flags, as every platform has them"
)]
pub struct KeyPressed {
    /// Shift held.
    pub shift: bool,
    /// Alt (⌥ on a Mac) held.
    pub alt: bool,
    /// Ctrl held.
    pub ctrl: bool,
    /// ⌘ on a Mac, the Windows key elsewhere.
    pub meta: bool,
    /// An input method (Japanese, Chinese, a dead key) is composing: the key is its, not ours.
    pub composing: bool,
    /// From a touch screen's own keyboard, not a hardware one: a phone's, or a tablet's.
    pub on_screen: bool,
}

/// What Enter does in the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EnterDoes {
    /// Sends what's written.
    Send,
    /// Starts a new line at the caret.
    NewLine,
    /// Nothing of ours: the input method takes it, to end what it's composing.
    Composing,
}

/// What Enter does in the composer: an input method's while it composes; a new line from a
/// touch screen's keyboard, whose Send button sends, as messaging apps on phones do; with
/// Shift, Alt, Ctrl or ⌘, a new line, as Slack does; otherwise it sends.
#[must_use]
pub fn enter(pressed: KeyPressed) -> EnterDoes {
    if pressed.composing {
        EnterDoes::Composing
    } else if pressed.on_screen || pressed.shift || pressed.alt || pressed.ctrl || pressed.meta {
        EnterDoes::NewLine
    } else {
        EnterDoes::Send
    }
}

/// What the composer holds as Up is pressed: whether it's empty, and whether it's already
/// correcting a message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(rename_all = "camelCase")]
pub struct Composing {
    /// Nothing written in it.
    pub empty: bool,
    /// It's correcting a message already.
    pub editing: bool,
}

/// Whether Up, pressed in the composer with `pressed`, corrects our last message in the
/// conversation ([`crate::chat::Chat::last_editable`]): while the composer is empty and not
/// correcting one already, with no modifier and no input method composing, as Slack does
/// (the user's choice, 20 Nov 2026). Otherwise it's the field's own, moving the caret.
#[must_use]
pub fn up_edits_last(composer: Composing, pressed: KeyPressed) -> bool {
    composer.empty
        && !composer.editing
        && !(pressed.composing || pressed.shift || pressed.alt || pressed.ctrl || pressed.meta)
}

/// What Escape does in the composer, as every app reads it ([`escape`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EscapeDoes {
    /// Closes the list of mentions or emoji it offers.
    ClosePopup,
    /// Stops correcting a message, giving back what was written before.
    StopEditing,
    /// Stops replying: what's written goes as an ordinary message.
    StopReplying,
    /// Nothing of the composer's: the app's own, as a desktop's marking the conversation
    /// read.
    Nothing,
}

/// What Escape does in the composer, the first that applies: closes the list it offers
/// (`popup`), else stops correcting a message (`editing`), else stops a reply (`replying`).
/// The same in every app with a keyboard.
#[must_use]
pub fn escape(popup: bool, composer: Composing, replying: bool) -> EscapeDoes {
    if popup {
        EscapeDoes::ClosePopup
    } else if composer.editing {
        EscapeDoes::StopEditing
    } else if replying {
        EscapeDoes::StopReplying
    } else {
        EscapeDoes::Nothing
    }
}

/// A key the list of mentions or emoji over the composer may take ([`in_list`]), as an app
/// names it from its own key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ListKey {
    /// The up arrow.
    Up,
    /// The down arrow.
    Down,
    /// Tab.
    Tab,
    /// Enter, or Return.
    Enter,
}

/// What a key does while the composer offers its list of mentions or emoji ([`in_list`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum ListDoes {
    /// Chooses the one above, round from the top to the bottom.
    Previous,
    /// Chooses the one below, round from the bottom to the top.
    Next,
    /// Writes the one chosen into the field, closing the list.
    Pick,
    /// Nothing of the list's: the field's own, as with no list (a new line, the caret).
    Field,
}

/// What `key`, pressed with `pressed` while the composer offers its list of mentions or
/// emoji, does: the arrows choose, and Tab or Enter writes the one chosen in, as in Slack.
/// With a modifier, from a touch screen's keyboard, or while an input method composes, it's
/// the field's: Shift+Enter still starts a new line. Escape closes the list ([`escape`]).
/// The same in every app with a keyboard; the desktop app's keymap (`mention::bind_keys`)
/// declares a copy, as GPUI binds keys ahead: change both together.
#[must_use]
pub fn in_list(key: ListKey, pressed: KeyPressed) -> ListDoes {
    if pressed.composing
        || pressed.on_screen
        || pressed.shift
        || pressed.alt
        || pressed.ctrl
        || pressed.meta
    {
        return ListDoes::Field;
    }
    match key {
        ListKey::Up => ListDoes::Previous,
        ListKey::Down => ListDoes::Next,
        ListKey::Tab | ListKey::Enter => ListDoes::Pick,
    }
}

/// Which of a list of `len` is chosen once `does` is done with `chosen` chosen: the one above
/// or below, round the ends, for [`ListDoes::Previous`] and [`ListDoes::Next`]; otherwise
/// `chosen`, kept inside a list that may have shrunk. 0 for an empty list.
#[must_use]
pub fn list_moved(chosen: usize, len: usize, does: ListDoes) -> usize {
    if len == 0 {
        return 0;
    }
    let chosen = chosen.min(len - 1);
    match does {
        ListDoes::Previous => (chosen + len - 1) % len,
        ListDoes::Next => (chosen + 1) % len,
        ListDoes::Pick | ListDoes::Field => chosen,
    }
}

/// Whether `key`, typed while nothing that takes text has the focus, goes into the composer
/// (focusing it) on a desktop or in a browser: a printable character with no modifier but
/// Shift. A shortcut (⌘K, Ctrl+F) stays a shortcut, and Space, Enter, the arrows and the
/// like stay the page's, for scrolling and buttons.
#[must_use]
pub fn types_into_composer(key: &str, pressed: KeyPressed) -> bool {
    let mut chars = key.chars();
    let one = matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control() && !c.is_whitespace());
    one && !(pressed.alt || pressed.ctrl || pressed.meta || pressed.composing)
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_list_takes_the_arrows_tab_and_enter_but_not_with_a_modifier() {
        let plain = KeyPressed::default();
        assert_eq!(in_list(ListKey::Up, plain), ListDoes::Previous);
        assert_eq!(in_list(ListKey::Down, plain), ListDoes::Next);
        assert_eq!(in_list(ListKey::Tab, plain), ListDoes::Pick);
        assert_eq!(in_list(ListKey::Enter, plain), ListDoes::Pick);
        let shift = KeyPressed {
            shift: true,
            ..plain
        };
        assert_eq!(in_list(ListKey::Enter, shift), ListDoes::Field);
        let composing = KeyPressed {
            composing: true,
            ..plain
        };
        assert_eq!(in_list(ListKey::Enter, composing), ListDoes::Field);
        let on_screen = KeyPressed {
            on_screen: true,
            ..plain
        };
        assert_eq!(in_list(ListKey::Enter, on_screen), ListDoes::Field);
    }

    #[test]
    fn the_list_moves_round_its_ends_and_keeps_inside_it() {
        assert_eq!(list_moved(0, 3, ListDoes::Previous), 2);
        assert_eq!(list_moved(2, 3, ListDoes::Next), 0);
        assert_eq!(list_moved(1, 3, ListDoes::Next), 2);
        assert_eq!(list_moved(5, 3, ListDoes::Pick), 2);
        assert_eq!(list_moved(4, 0, ListDoes::Next), 0);
    }

    #[test]
    fn escape_closes_the_popup_then_stops_an_edit_then_a_reply() {
        let editing = Composing {
            empty: false,
            editing: true,
        };
        assert_eq!(escape(true, editing, true), EscapeDoes::ClosePopup);
        assert_eq!(escape(false, editing, true), EscapeDoes::StopEditing);
        assert_eq!(
            escape(false, Composing::default(), true),
            EscapeDoes::StopReplying
        );
        assert_eq!(
            escape(false, Composing::default(), false),
            EscapeDoes::Nothing
        );
    }
    use super::*;

    const PLAIN: KeyPressed = KeyPressed {
        shift: false,
        alt: false,
        ctrl: false,
        meta: false,
        composing: false,
        on_screen: false,
    };

    #[test]
    fn up_edits_the_last_message_only_from_an_empty_composer_with_no_modifier() {
        let plain = KeyPressed::default();
        let empty = Composing {
            empty: true,
            editing: false,
        };
        assert!(up_edits_last(empty, plain));
        assert!(!up_edits_last(Composing::default(), plain));
        // Correcting one already, Up is the field's.
        let editing = Composing {
            editing: true,
            ..empty
        };
        assert!(!up_edits_last(editing, plain));
        for held in [
            KeyPressed {
                shift: true,
                ..plain
            },
            KeyPressed { alt: true, ..plain },
            KeyPressed {
                composing: true,
                ..plain
            },
        ] {
            assert!(!up_edits_last(empty, held));
        }
    }

    #[test]
    fn enter_sends_and_a_modifier_makes_a_new_line() {
        assert_eq!(enter(PLAIN), EnterDoes::Send);
        for held in [
            KeyPressed {
                shift: true,
                ..PLAIN
            },
            KeyPressed { alt: true, ..PLAIN },
            KeyPressed {
                ctrl: true,
                ..PLAIN
            },
            KeyPressed {
                meta: true,
                ..PLAIN
            },
        ] {
            assert_eq!(enter(held), EnterDoes::NewLine, "{held:?}");
        }
    }

    #[test]
    fn enter_ending_a_composition_is_the_input_methods() {
        assert_eq!(
            enter(KeyPressed {
                composing: true,
                ..PLAIN
            }),
            EnterDoes::Composing
        );
        assert_eq!(
            enter(KeyPressed {
                composing: true,
                on_screen: true,
                ..PLAIN
            }),
            EnterDoes::Composing
        );
    }

    #[test]
    fn a_touch_keyboards_return_makes_a_new_line() {
        assert_eq!(
            enter(KeyPressed {
                on_screen: true,
                ..PLAIN
            }),
            EnterDoes::NewLine
        );
    }

    #[test]
    fn printable_keys_go_to_the_composer_and_shortcuts_dont() {
        assert!(types_into_composer("a", PLAIN));
        assert!(types_into_composer(
            "A",
            KeyPressed {
                shift: true,
                ..PLAIN
            }
        ));
        assert!(types_into_composer("é", PLAIN));
        assert!(types_into_composer("/", PLAIN));
        assert!(!types_into_composer(
            "k",
            KeyPressed {
                meta: true,
                ..PLAIN
            }
        ));
        assert!(!types_into_composer(
            "f",
            KeyPressed {
                ctrl: true,
                ..PLAIN
            }
        ));
        assert!(!types_into_composer("a", KeyPressed { alt: true, ..PLAIN }));
        assert!(!types_into_composer(" ", PLAIN));
        assert!(!types_into_composer("Enter", PLAIN));
        assert!(!types_into_composer("ArrowDown", PLAIN));
        assert!(!types_into_composer("\t", PLAIN));
        assert!(!types_into_composer(
            "a",
            KeyPressed {
                composing: true,
                ..PLAIN
            }
        ));
    }
}
