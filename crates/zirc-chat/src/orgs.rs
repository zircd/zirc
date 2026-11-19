//! A chat per organisation, for apps signed in to several, as Slack holds several
//! workspaces. Each organisation has a server of its own, so its chat is a [`Chat`] like any
//! other, with its own connection, history and people; [`Orgs`] holds them in the order
//! signed in to, one of them selected, and says what's unread in each for a switcher's
//! badges. It's generic over what an app keeps per organisation, such as a session that
//! wraps the chat, so long as that gives its [`Chat`].
//!
//! Organisations are named as every client names them, `acme` for `acme.irc.zirc.dev`
//! (zirc-core's `settings::org_name`), so an app's keys agree with `config.toml`'s and its
//! sign-ins'. With one organisation an app shows no switcher: someone in one organisation
//! sees no change.

use crate::chat::{Chat, Conversation, Level, badge_label, initials};

/// One `T` per organisation, in the order they were added, one of them selected unless
/// there are none.
#[derive(Debug, Clone)]
pub struct Orgs<T> {
    orgs: Vec<(String, T)>,
    selected: usize,
}

impl<T> Default for Orgs<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Orgs<T> {
    /// None yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            orgs: Vec::new(),
            selected: 0,
        }
    }

    /// Adds `it` for the organisation `org` after the others, or replaces that
    /// organisation's where it is, staying selected if it was. The first added is selected.
    pub fn insert(&mut self, org: impl Into<String>, it: T) {
        let org = org.into();
        match self.orgs.iter_mut().find(|(name, _)| *name == org) {
            Some((_, old)) => *old = it,
            None => self.orgs.push((org, it)),
        }
    }

    /// Takes the organisation `org` out. When it was selected, the next one is, or else the
    /// one before.
    pub fn remove(&mut self, org: &str) -> Option<T> {
        let at = self.position(org)?;
        let (_, it) = self.orgs.remove(at);
        if at < self.selected || (at == self.selected && self.selected == self.orgs.len()) {
            self.selected = self.selected.saturating_sub(1);
        }
        Some(it)
    }

    /// The organisation `org`'s.
    #[must_use]
    pub fn get(&self, org: &str) -> Option<&T> {
        self.orgs
            .iter()
            .find(|(name, _)| name == org)
            .map(|(_, it)| it)
    }

    /// The organisation `org`'s, to change.
    pub fn get_mut(&mut self, org: &str) -> Option<&mut T> {
        self.orgs
            .iter_mut()
            .find(|(name, _)| name == org)
            .map(|(_, it)| it)
    }

    /// Selects the organisation `org`. Answers whether there is one.
    pub fn select(&mut self, org: &str) -> bool {
        match self.position(org) {
            Some(at) => {
                self.selected = at;
                true
            }
            None => false,
        }
    }

    /// The organisation selected and its `T`: `None` only when there are none.
    #[must_use]
    pub fn selected(&self) -> Option<(&str, &T)> {
        self.orgs
            .get(self.selected)
            .map(|(name, it)| (name.as_str(), it))
    }

    /// The organisation selected and its `T`, to change.
    pub fn selected_mut(&mut self) -> Option<(&str, &mut T)> {
        self.orgs
            .get_mut(self.selected)
            .map(|(name, it)| (name.as_str(), it))
    }

    /// Each organisation and its `T`, in the order added.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.orgs.iter().map(|(name, it)| (name.as_str(), it))
    }

    /// Each organisation and its `T`, to change.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut T)> {
        self.orgs.iter_mut().map(|(name, it)| (name.as_str(), it))
    }

    /// How many organisations: an app shows a switcher when there's more than one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.orgs.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orgs.is_empty()
    }

    fn position(&self, org: &str) -> Option<usize> {
        self.orgs.iter().position(|(name, _)| name == org)
    }
}

/// What's unread, for a badge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unread {
    /// Messages from others not read yet.
    pub unread: usize,
    /// What its badge counts, as the rows' badges add up ([`Chat::badge_total`]): what
    /// mentions us in a channel, and every message in a direct or group conversation, which
    /// are meant for us.
    pub mentions: usize,
}

impl std::ops::Add for Unread {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            unread: self.unread + other.unread,
            mentions: self.mentions + other.mentions,
        }
    }
}

impl Chat {
    /// What's unread in the conversations `counts` says count, such as all but those muted.
    pub fn unread(&self, counts: impl Fn(&Conversation) -> bool) -> Unread {
        self.conversations()
            .iter()
            .filter(|conversation| counts(conversation))
            .map(|conversation| Unread {
                unread: conversation.unread,
                mentions: conversation.mentions,
            })
            .fold(Unread::default(), std::ops::Add::add)
    }
}

/// Where an organisation stands on a rail or a switcher, as every app says it. Connected is
/// the normal case and is drawn with no mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgState {
    /// Signed in to the server and connected.
    Connected,
    /// Connecting, or reconnecting.
    Connecting,
    /// A new organisation whose server is still being made.
    SettingUp,
    /// The server can't be reached; the app retries.
    Offline,
    /// The server refused this device's sign-in, such as after a rename.
    SignInAgain,
    /// The server no longer takes this person: their membership was removed. No retrying.
    AccessEnded,
}

impl OrgState {
    /// What a rail, a tooltip or a screen reader says of it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Connecting => "Connecting…",
            Self::SettingUp => "Setting up",
            Self::Offline => "Offline",
            Self::SignInAgain => "Sign in again",
            Self::AccessEnded => "Access ended",
        }
    }
}

/// The colours an organisation's tile is picked from, until it has a picture: a fixed set every
/// app draws alike, as `#rrggbb`. Each is a saturated hue, never grey (grey is an organisation
/// the person is no longer in), and keeps at least 4.5:1 with white initials, the same in both
/// themes: blue, green, purple, orange, pink, teal, amber, lime.
pub const ORG_PALETTE: [&str; ORG_COLOURS as usize] = [
    "#3b4fd8", "#0f7a5f", "#8e3fb0", "#b4461a", "#b0306b", "#0e7490", "#8a5a00", "#3f7a0f",
];

/// How many colours [`ORG_PALETTE`] has.
pub const ORG_COLOURS: u8 = 8;

/// The colour an organisation's tile has, as its place in [`ORG_PALETTE`], picked by its name
/// so every app and every device agree: FNV-1a of the name, lowercased.
#[must_use]
pub fn org_colour(org: &str) -> u8 {
    let hash = org
        .to_lowercase()
        .bytes()
        .fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
    u8::try_from(hash % u32::from(ORG_COLOURS)).unwrap_or_default()
}

/// What an organisation's tile shows without a picture: the first letter of its name, or of
/// its first two words, `Acme` → `A`, `Zirc Labs` → `ZL`, as an avatar's [`initials`] are,
/// so a name known only by its slug, `acme-labs` → `AL`, isn't another `A`.
#[must_use]
pub fn org_initials(name: &str) -> String {
    initials(name)
}

/// What an organisation's badge says on a rail, a switcher or a tab, as every app shows it:
/// what it counts ([`Chat::org_badge`]) capped as every badge is ([`badge_label`]), `3`,
/// `99+`; none for nothing, nor for one the person is no longer in, whatever it held.
#[must_use]
pub fn org_badge_label(state: OrgState, mentions: usize) -> Option<String> {
    (state != OrgState::AccessEnded && mentions > 0).then(|| badge_label(mentions))
}

/// Whether an organisation shows a dot in place of its badge, as every app does: something
/// unread there but nothing its badge counts ([`org_badge_label`]), and never for one the
/// person is no longer in.
#[must_use]
pub fn org_dot(state: OrgState, unread: Unread) -> bool {
    state != OrgState::AccessEnded && unread.mentions == 0 && unread.unread > 0
}

/// What the composer says while empty, in the conversation shown as `short`: `Message #api`,
/// and with several organisations the one it goes to, `Message #api in Acme`, so nothing is
/// said in the wrong one.
#[must_use]
pub fn composer_placeholder(short: &str, org: Option<&str>) -> String {
    match org {
        Some(org) => format!("Message {short} in {org}"),
        None => format!("Message {short}"),
    }
}

impl Chat {
    /// The organisation's badge on a rail, a switcher or the app's icon, as every app counts
    /// it: what's unread in the conversations that notify (all but those set to nothing), and
    /// the badge the sum of its rows' ([`Chat::badge_total`]), as decided ("Same as the rows").
    #[must_use]
    pub fn org_badge(&self) -> Unread {
        Unread {
            mentions: self.badge_total(),
            ..self.unread(|conversation| self.level(&conversation.name) != Level::Nothing)
        }
    }

    /// The same for an organisation that stands as `state`: nothing for one the person is no
    /// longer in, whatever it held.
    #[must_use]
    pub fn org_badge_as(&self, state: OrgState) -> Unread {
        if state == OrgState::AccessEnded {
            Unread::default()
        } else {
            self.org_badge()
        }
    }
}

impl<T: AsRef<Chat>> Orgs<T> {
    /// What's unread in each organisation, in the conversations `counts` says count for it.
    pub fn unread(&self, counts: impl Fn(&str, &Conversation) -> bool) -> Vec<(&str, Unread)> {
        self.iter()
            .map(|(org, it)| {
                (
                    org,
                    it.as_ref().unread(|conversation| counts(org, conversation)),
                )
            })
            .collect()
    }

    /// What's unread in the organisations not selected, for a badge that says there's news
    /// elsewhere.
    pub fn unread_elsewhere(&self, counts: impl Fn(&str, &Conversation) -> bool) -> Unread {
        let selected = self.selected().map(|(org, _)| org);
        self.unread(counts)
            .into_iter()
            .filter(|(org, _)| Some(*org) != selected)
            .map(|(_, unread)| unread)
            .fold(Unread::default(), std::ops::Add::add)
    }
}

impl AsRef<Self> for Chat {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AsMut<Self> for Chat {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

// The words of signing out, for every app, the web chat's too: zirc-core's `account_text`
// says them through these.

/// A sign-out sheet: what it asks, what it says, and the destructive button. Every app reads
/// this record, the phones through `UniFFI` and the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct SignOutSheet {
    /// The title.
    pub title: String,
    /// The body.
    pub body: String,
    /// The confirm button; the other is always Cancel.
    pub confirm: String,
}

/// An organisation as people read its name: `acme` is Acme, `zirc-labs` Zirc Labs, a host as it is. The core
/// has the slug only, so this is how every app names it until organisations carry a name.
#[must_use]
pub fn display_name(org: &str) -> String {
    // A self-hosted organisation is its host, which is read as it is.
    if org.contains('.') {
        return org.to_owned();
    }
    org.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What signs out, where the sheet before it or the row has said of what.
pub const SIGN_OUT: &str = "Sign out";

/// An organisation's menu item that signs out of it on this device: "Sign out of Acme", by
/// the name it shows as.
#[must_use]
pub fn sign_out_label(name: &str) -> String {
    format!("Sign out of {name}")
}

/// What an organisation's card offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    /// Signing in to the organisation again, the browser handshake for it alone.
    SignInAgain,
    /// Signing out of it here, which takes it off the device.
    Remove,
}

/// The card an organisation shows in place of its conversations when it can't be used, the
/// same in every app ([`org_card`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// `Setting up Acme`.
    pub title: String,
    /// What it means.
    pub body: String,
    /// Its buttons, the main one first.
    pub actions: Vec<CardAction>,
    /// The organisation's name as people read it, which its buttons say.
    pub name: String,
}

impl Card {
    /// What `action`'s button says: "Sign in again", or "Sign out of Acme", as the
    /// organisation's own sign-out item says it in every app, since that's what it does.
    #[must_use]
    pub fn label(&self, action: CardAction) -> String {
        match action {
            CardAction::SignInAgain => "Sign in again".to_owned(),
            CardAction::Remove => sign_out_label(&self.name),
        }
    }
}

/// The card of the organisation people read as `name`, standing as `state`, if one: setting
/// up, signing in again, or access ended. Connected, connecting and offline show the
/// organisation itself, with the offline bar.
#[must_use]
pub fn org_card(state: OrgState, name: &str) -> Option<Card> {
    let card = |title: String, body: &str, actions: Vec<CardAction>| Card {
        title,
        body: body.to_owned(),
        actions,
        name: name.to_owned(),
    };
    match state {
        OrgState::SettingUp => Some(card(
            format!("Setting up {name}"),
            "This usually takes about a minute. You can switch to another organisation \
             meanwhile.",
            Vec::new(),
        )),
        OrgState::SignInAgain => Some(Card {
            body: format!(
                "{name} didn't accept this device's sign-in. Your messages here are kept."
            ),
            ..card(
                format!("Sign in to {name} again"),
                "",
                vec![CardAction::SignInAgain, CardAction::Remove],
            )
        }),
        OrgState::AccessEnded => Some(card(
            format!("You're no longer in {name}"),
            "An owner removed you, or the organisation was closed. Nothing more will arrive \
             from it.",
            vec![CardAction::Remove],
        )),
        OrgState::Connected | OrgState::Connecting | OrgState::Offline => None,
    }
}

/// What every app says once it has signed out of `org` (the organisation's slug, or a
/// self-hosted server's host): `Signed out of Acme.`, as decided ("The org's name").
#[must_use]
pub fn signed_out(org: &str) -> String {
    format!("Signed out of {}.", display_name(org))
}

/// What the sheet for signing out of `org` says, `others` being the organisations that stay:
/// one of several, or the last (which also signs out of zirc.dev, unless the organisation is
/// self-hosted and has none). The apps' sign-out sheets and `zirc logout` say it here
/// once.
#[must_use]
pub fn sign_out_sheet(org: &str, others: &[String], self_hosted: bool) -> SignOutSheet {
    let name = display_name(org);
    let title = format!("Sign out of {name}?");
    if !others.is_empty() {
        let stay = match others {
            [one] => format!("You stay signed in to {}.", display_name(one)),
            _ => "You stay signed in to your other organisations.".to_owned(),
        };
        return SignOutSheet {
            title,
            body: format!(
                "{name}'s messages, notifications and settings leave this device. {stay}"
            ),
            confirm: sign_out_label(&name),
        };
    }
    let also = if self_hosted {
        String::new()
    } else {
        ", so this also signs you out of zirc.dev here".to_owned()
    };
    SignOutSheet {
        title,
        body: format!("{name} is the only organisation on this device{also}."),
        confirm: SIGN_OUT.to_owned(),
    }
}

/// The title (with `?`) and button of the question that signs out of every organisation at
/// once: [`crate::chat::SIGN_OUT_EVERYTHING`] without its ellipsis, which is what asks it.
pub const SIGN_OUT_EVERYTHING_NAME: &str = "Sign out of everything";

/// Where signing out of everything forgets the sign-ins: on this device (an app), or in this
/// browser (the web chat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignOutPlace {
    /// An app on a phone or a computer.
    Device,
    /// The web chat, in a browser.
    Browser,
}

/// `names` as a sentence list: `a`, `a and b`, `a, b and c`.
fn sentence(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// How many organisations the sheet for signing out of everything names before `N more`.
const NAMED: usize = 3;

/// What the sheet for signing out of everything at `place` says of the organisations people
/// read as `names`: up to three of them, then how many more, and zirc.dev last when
/// `zirc_dev` (any of them is on it).
#[must_use]
pub fn sign_out_everything_sheet(
    names: &[String],
    zirc_dev: bool,
    place: SignOutPlace,
) -> SignOutSheet {
    let mut said: Vec<String> = names.iter().take(NAMED).cloned().collect();
    if names.len() > NAMED {
        said.push(format!("{} more", names.len() - NAMED));
    }
    if zirc_dev {
        said.push("zirc.dev".to_owned());
    }
    let place = match place {
        SignOutPlace::Device => "on this device",
        SignOutPlace::Browser => "in this browser",
    };
    SignOutSheet {
        title: format!("{SIGN_OUT_EVERYTHING_NAME}?"),
        body: format!("Signs you out of {} {place}.", sentence(&said)),
        confirm: SIGN_OUT_EVERYTHING_NAME.to_owned(),
    }
}

/// What a screen reader says of an organisation's tile, the organisation people read as
/// `name`, with `unread` there, standing as `state`: `Acme, 3 mentions, connecting`, or
/// `Acme, unread messages` when nothing mentions the person.
#[must_use]
pub fn tile_a11y(name: &str, unread: Unread, state: OrgState) -> String {
    let mut said = vec![name.to_owned()];
    match unread.mentions {
        0 if unread.unread > 0 => said.push("unread messages".to_owned()),
        0 => {}
        n => said.push(crate::chat::mentions_label(n)),
    }
    if state != OrgState::Connected {
        said.push(state.label().trim_end_matches('…').to_lowercase());
    }
    said.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_organisations_badge_is_capped_and_gone_once_access_ended() {
        assert_eq!(org_badge_label(OrgState::Connected, 0), None);
        assert_eq!(org_badge_label(OrgState::Offline, 3).as_deref(), Some("3"));
        assert_eq!(
            org_badge_label(OrgState::Connected, 140).as_deref(),
            Some("99+")
        );
        assert_eq!(org_badge_label(OrgState::AccessEnded, 3), None);
        let unread = |unread, mentions| Unread { unread, mentions };
        assert!(org_dot(OrgState::Connected, unread(2, 0)));
        assert!(
            !org_dot(OrgState::Connected, unread(2, 1)),
            "the badge says it"
        );
        assert!(!org_dot(OrgState::Connected, unread(0, 0)));
        assert!(!org_dot(OrgState::AccessEnded, unread(2, 0)));
    }

    #[test]
    fn signing_out_of_everything_names_three_then_how_many_more_then_zirc_dev() {
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        let sheet =
            sign_out_everything_sheet(&names(&["Acme", "Zirc"]), true, SignOutPlace::Browser);
        assert_eq!(sheet.title, "Sign out of everything?");
        assert_eq!(
            sheet.body,
            "Signs you out of Acme, Zirc and zirc.dev in this browser."
        );
        assert_eq!(sheet.confirm, "Sign out of everything");
        assert_eq!(
            sign_out_everything_sheet(
                &names(&["A", "B", "C", "D", "E"]),
                true,
                SignOutPlace::Device
            )
            .body,
            "Signs you out of A, B, C, 2 more and zirc.dev on this device."
        );
        assert_eq!(
            sign_out_everything_sheet(&[], true, SignOutPlace::Browser).body,
            "Signs you out of zirc.dev in this browser."
        );
        assert_eq!(
            sign_out_everything_sheet(&names(&["chat.example.org"]), false, SignOutPlace::Device)
                .body,
            "Signs you out of chat.example.org on this device."
        );
    }

    #[test]
    fn a_tile_says_its_mentions_or_unread_and_how_it_stands() {
        let unread = |unread, mentions| Unread { unread, mentions };
        assert_eq!(
            tile_a11y("Acme", unread(5, 3), OrgState::Connecting),
            "Acme, 3 mentions, connecting"
        );
        assert_eq!(
            tile_a11y("Acme", unread(2, 0), OrgState::Connected),
            "Acme, unread messages"
        );
        assert_eq!(
            tile_a11y("Acme", unread(0, 1), OrgState::SignInAgain),
            "Acme, 1 mention, sign in again"
        );
        assert_eq!(tile_a11y("Acme", unread(0, 0), OrgState::Connected), "Acme");
    }
    use crate::chat::Options;

    fn names(orgs: &Orgs<u32>) -> Vec<&str> {
        orgs.iter().map(|(org, _)| org).collect()
    }

    #[test]
    fn keeps_one_per_organisation_with_one_selected() {
        let mut orgs = Orgs::new();
        assert!(orgs.selected().is_none());
        orgs.insert("acme", 1);
        orgs.insert("globex", 2);
        orgs.insert("initech", 3);
        assert_eq!(orgs.selected(), Some(("acme", &1)));
        assert!(orgs.select("globex"));
        assert!(!orgs.select("umbrella"));
        // Inserting again keeps its place and the selection.
        orgs.insert("globex", 20);
        assert_eq!(names(&orgs), ["acme", "globex", "initech"]);
        assert_eq!(orgs.selected(), Some(("globex", &20)));
        *orgs.get_mut("acme").unwrap() = 10;
        assert_eq!(orgs.get("acme"), Some(&10));

        // Removing the selected one selects the next, or else the one before.
        assert_eq!(orgs.remove("globex"), Some(20));
        assert_eq!(orgs.selected(), Some(("initech", &3)));
        assert_eq!(orgs.remove("initech"), Some(3));
        assert_eq!(orgs.selected(), Some(("acme", &10)));
        // Removing one before the selected one keeps the selection.
        orgs.insert("globex", 2);
        orgs.select("globex");
        orgs.remove("acme");
        assert_eq!(orgs.selected(), Some(("globex", &2)));
        orgs.remove("globex");
        assert!(orgs.selected().is_none() && orgs.is_empty());
    }

    /// A chat with `unread` messages from others in each of `#general` and `#noise`, the
    /// first `mentions` of them naming us.
    fn chat(unread: usize, mentions: usize) -> Chat {
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        let _ = chat.connected(crate::chat::Link::new("me", |_| false, None));
        for channel in ["#general", "#noise"] {
            let _ = chat.receive(format!(":me!u@h JOIN {channel}").parse().unwrap());
        }
        // Looking at the server's conversation, so what's said in the channels is unread.
        let _ = chat.select(0);
        for channel in ["#general", "#noise"] {
            for at in 0..unread {
                let text = if at < mentions { "me: look" } else { "hello" };
                let line = format!(":alice!u@h PRIVMSG {channel} :{text}");
                let _ = chat.receive(line.parse().unwrap());
            }
        }
        chat
    }

    #[test]
    fn says_whats_unread_in_each_organisation_and_elsewhere() {
        let mut orgs = Orgs::new();
        orgs.insert("acme", chat(3, 1));
        orgs.insert("globex", chat(2, 0));
        let all = |_: &str, _: &Conversation| true;
        let unread = orgs.unread(all);
        assert_eq!(
            unread,
            [
                (
                    "acme",
                    Unread {
                        unread: 6,
                        mentions: 2
                    }
                ),
                (
                    "globex",
                    Unread {
                        unread: 4,
                        mentions: 0
                    }
                )
            ]
        );
        assert_eq!(
            orgs.unread_elsewhere(all),
            Unread {
                unread: 4,
                mentions: 0
            }
        );
        // Mutes are each organisation's: globex's #noise is muted, acme's isn't.
        let unmuted = |org: &str, conversation: &Conversation| {
            !(org == "globex" && conversation.name == "#noise")
        };
        assert_eq!(
            orgs.unread_elsewhere(unmuted),
            Unread {
                unread: 2,
                mentions: 0
            }
        );
        orgs.select("globex");
        assert_eq!(
            orgs.unread_elsewhere(unmuted),
            Unread {
                unread: 6,
                mentions: 2
            }
        );
    }

    #[test]
    fn an_organisation_s_badge_is_its_rows_and_leaves_out_what_is_set_to_nothing() {
        let mut chat = chat(3, 1);
        assert_eq!(
            chat.org_badge(),
            Unread {
                unread: 6,
                mentions: 2
            }
        );
        chat.set_level("#noise", crate::chat::Level::Nothing);
        // Its unread leave the count, but a mention there still shows on its row, and so in the
        // organisation's badge, the sum of its rows'.
        assert_eq!(
            chat.org_badge(),
            Unread {
                unread: 3,
                mentions: 2
            }
        );
    }

    #[test]
    fn an_organisation_s_tile_and_composer_name_it_alike_everywhere() {
        assert_eq!(org_initials("Acme"), "A");
        assert_eq!(org_initials("zirc labs"), "ZL");
        assert_eq!(org_initials("  Big  Fat Co "), "BF");
        // Known by its slug alone, as before zirc.dev names it: never the same as Acme's.
        assert_eq!(org_initials("acme-labs"), "AL");
        assert_eq!(org_initials(""), "");
        // The same name, any case, the same colour, always one of the set.
        assert_eq!(org_colour("acme"), org_colour("Acme"));
        let colours: std::collections::HashSet<u8> =
            ["acme", "zirc", "beta", "globex", "initech", "umbrella"]
                .into_iter()
                .map(org_colour)
                .collect();
        assert!(colours.iter().all(|colour| *colour < ORG_COLOURS));
        assert!(colours.len() > 1, "{colours:?}");
        // Pinned: a name's place in the palette is what every app on every device draws, so
        // changing the hash or the palette's order is a deliberate break, and these say so.
        // The rule is FNV-1a (32 bit) of the lowercased name, mod 8; initials are the first
        // letters of the first two words (the design's examples: Acme, Zirc Labs).
        for (name, place) in [
            ("acme", 7),
            ("Zirc", 3),
            ("globex", 6),
            ("chat.example.org", 7),
        ] {
            assert_eq!(org_colour(name), place, "{name}");
        }
        assert_eq!(org_initials("Zirc Labs"), "ZL");
        assert_eq!(composer_placeholder("#api", None), "Message #api");
        assert_eq!(
            composer_placeholder("Leo Okafor", Some("Acme")),
            "Message Leo Okafor in Acme"
        );
    }

    /// WCAG's contrast of `hex` against white, and its saturation, from 0 to 1.
    fn contrast_and_saturation(hex: &str) -> (f64, f64) {
        let channel =
            |at: usize| f64::from(u8::from_str_radix(&hex[at..at + 2], 16).unwrap()) / 255.0;
        let (r, g, b) = (channel(1), channel(3), channel(5));
        let linear = |c: f64| {
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
        let (most, least) = (r.max(g).max(b), r.min(g).min(b));
        let lightness = f64::midpoint(most, least);
        let saturation = if (most - least).abs() < f64::EPSILON {
            0.0
        } else {
            (most - least) / (1.0 - (2.0 * lightness - 1.0).abs())
        };
        (1.05 / (luminance + 0.05), saturation)
    }

    #[test]
    fn every_tile_colour_is_a_hue_white_initials_read_on() {
        for hex in ORG_PALETTE {
            let (contrast, saturation) = contrast_and_saturation(hex);
            assert!(contrast >= 4.5, "{hex}: {contrast:.2}:1 with white");
            assert!(saturation >= 0.4, "{hex} is greyish: {saturation:.2}");
        }
    }
}
