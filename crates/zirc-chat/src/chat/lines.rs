//! A conversation's lines as every app's records carry them: what [`Chat::items`] lays out,
//! each with its key, times on the view's clock, its day divider, who said it and their face,
//! its rich text cut when long, its reactions and quote, and a system line's or a file's
//! parts, all worded here. The phones take them over UniFFI and the web client as JSON, and
//! only draw them.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};
use serde::Serialize;
use ts_rs::TS;

use super::conversation::{Conversation, Entry, agent, fold};
use super::face::{Face, LineSpeaker};
use super::flat::{RichBlock, RichSpan, is_plain};
use super::rich::{Block, Span, SpanKind};
use super::view::{Item, Landing, Parts};
use super::{Chat, Icon, Presence, Standing, activity_conversation, file, first_link, link_blocks};

/// The largest an image or a video's poster is drawn, in points: wider ones are fitted.
pub const MEDIA_WIDTH: u32 = 360;

/// The tallest an image or a video's poster is drawn, in points.
pub const MEDIA_HEIGHT: u32 = 240;

/// How many times an animated picture, such as a GIF, plays before it rests on its last
/// frame, as Slack's do; a tap on one that rests plays it again (the user's choice, 13 Nov
/// 2026, for every app).
pub const ANIMATION_PLAYS: u32 = 3;

/// How a video plays in the apps that play it themselves, every one (the user's choice, 17
/// Nov 2026: in the app where there's a player, the GPUI desktop opening the browser): the
/// copy zirc.dev makes at most [`VideoPlan::lines`] high, or the screen's pixels if fewer,
/// asked for again every [`VideoPlan::retry_ms`] while it's being made, [`VideoPlan::tries`]
/// times, about two minutes, before saying zirc.dev couldn't be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPlan {
    /// The tallest copy asked for, in pixels: 720.
    pub lines: u32,
    /// How long to wait before asking again while it's being made, as zirc.dev's
    /// Retry-After says.
    pub retry_ms: u64,
    /// How many times to ask.
    pub tries: u32,
}

/// How every app plays a video: see [`VideoPlan`].
pub const VIDEO_PLAN: VideoPlan = VideoPlan {
    lines: 720,
    retry_ms: 5_000,
    tries: 24,
};

/// What a video's player says while zirc.dev makes the copy it plays.
pub const VIDEO_MAKING: &str = "Getting the video ready…";

/// What a video's player says when zirc.dev couldn't be reached to play it.
pub const VIDEO_UNREACHABLE: &str = "Couldn't reach zirc.dev to play it";

/// What sort of line it is, for how it's drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum LineKind {
    /// Something someone said.
    Chat,
    /// A `/me` action: `alice waves`.
    Action,
    /// A notice: a message that expects no answer.
    Notice,
    /// A zirc event, or several people changing the same files: [`Line::system`] has its
    /// parts.
    Event,
    /// How a landing went, or any card: one line for each, which later events update.
    /// [`Line::system`] has its outcome and buttons, [`Line::rich`] its words.
    Landing,
    /// People who came and went, as one quiet line.
    Membership,
    /// A quiet line: a topic, a new nick, someone removed, what the app says, or a file
    /// shared from elsewhere, which isn't loaded.
    Note,
    /// A file someone shared: [`Line::file`] has what to draw.
    File,
}

/// How a line's row is drawn, decided here for every app, which draws the shape it's given
/// and never decides it from the line's kind, icon or parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum RowShape {
    /// What someone said or did, or a file they shared, under who they are.
    Message,
    /// An event, a landing or a card, drawn as a message is: its actor's face and name, then
    /// its [`SystemLine::body`].
    System,
    /// One muted line: people coming and going, with [`SystemLine::glyph`] as its marker and
    /// its time, the user's choice for every app on 16 Nov 2026; or a note, which has neither.
    Quiet,
    /// An agent's routine work as one muted line, [`SystemLine::brief`], which a click or a
    /// tap opens to its [`RowShape::System`] row; the app keeps which it opened.
    Brief,
    /// A card's update said further down: one muted line, [`Line::text`], which brings the
    /// card ([`SystemLine::points_to`]) into view when clicked or tapped.
    Pointer,
}

impl RowShape {
    /// The shape of `line`'s row, from what it is.
    fn of(line: &Line) -> Self {
        match (&line.system, &line.speaker) {
            (Some(system), _) if system.points_to.is_some() => Self::Pointer,
            (Some(system), Some(_)) if system.brief.is_some() => Self::Brief,
            _ if matches!(line.kind, LineKind::Membership | LineKind::Note) => Self::Quiet,
            (_, None) => Self::Quiet,
            (Some(_), Some(_)) => Self::System,
            (None, Some(_)) => Self::Message,
        }
    }
}

/// What a [`RowShape::System`] row draws under its head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum SystemBody {
    /// A landing's card: [`Line::rich`] is its line, with [`SystemLine::outcome`] and
    /// [`SystemLine::buttons`].
    Card,
    /// Words, as a message's are: [`Line::rich`] and [`Line::more`], a long one cut. A
    /// post's words, `→ zirc-chat` first when it's to an agent session; an older landing's.
    Blocks,
    /// Its parts: [`SystemLine::lead`], its [`SystemLine::details`], and its paths when
    /// [`SystemLine::paths_listed`].
    Parts,
}

/// One line of a conversation, as every app draws it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a fact of its own about the line"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Line {
    /// What stays the same for it however the list changes: the server's id, or else its
    /// time and how many id-less lines came before it at that time, `1730289600000:0`.
    pub key: String,
    /// The server's id, which reactions and replies name.
    pub id: Option<String>,
    /// In an agent session's conversation ([`Chat::agent_lines`]), the room a line of its
    /// activity was said in, `#web/agents`, for the muted chip after its time.
    pub room: Option<String>,
    /// When, in milliseconds since 1970.
    pub time_ms: i64,
    /// That on the view's clock: `15:48` ([`super::clock`]).
    pub clock: String,
    /// The day divider above it, `Today` or `Thursday 29 October 2026`, when it's the first
    /// line of its day ([`super::day_label`]).
    pub day: Option<String>,
    /// What sort of line it is.
    pub kind: LineKind,
    /// How its row is drawn.
    pub shape: RowShape,
    /// Whether it carries on a run from the same sender, so its avatar and name go.
    pub continues: bool,
    /// Who: their nick, which replies, mentions and their card name. Empty for what no one
    /// said, such as people coming and going.
    pub from: String,
    /// Who they are, as its head shows them; `None` for a quiet line.
    pub speaker: Option<LineSpeaker>,
    /// Their dot: here in the conversation, away or gone; `None` for a quiet line.
    pub presence: Option<Presence>,
    /// Whether they're a guest, let into a public channel without an account.
    pub guest: bool,
    /// What it says, whole, for copying and the menu.
    pub text: String,
    /// What it says as blocks to draw: code, quotes, lists, bold and the like. Empty when
    /// it's one plain paragraph shown whole, which [`Line::text`] says; never for a post (an
    /// event with [`Icon::Post`] and words), whose words these are, `→ zirc-chat` first when
    /// it's to an agent session ([`SystemBody::Blocks`]).
    pub rich: Vec<RichBlock>,
    /// The first web address it has, for Copy link: a landing's page, else its build's; a
    /// shared file's own.
    pub link: Option<String>,
    /// Whether it names us, so it stands out: never in a one-to-one conversation, where
    /// every line is for us.
    pub mention: bool,
    /// Whether it's our own.
    pub mine: bool,
    /// Whether its author corrected it, which apps say after its words as
    /// [`super::EDITED`] does.
    pub edited: bool,
    /// Its reactions, in the order given.
    pub reactions: Vec<LineReaction>,
    /// For a reply, what it answers, to show above it.
    pub quote: Option<LineQuote>,
    /// For a long message, its one "Show N more lines" control: [`Line::rich`] is then its
    /// start, or all of it while it's open ([`Chat::body`]).
    pub more: Option<LineMore>,
    /// For an event, a landing or people coming and going, its parts.
    pub system: Option<SystemLine>,
    /// For a file, what to draw.
    pub file: Option<LineFile>,
    /// What can be done with it, which its hover bar, menu and long-press sheet offer.
    pub actions: LineActions,
}

/// What can be done with a line, which every app's hover bar, right-click menu and
/// long-press sheet offer, and only these. An agent's event or a landing's card offers what
/// a message does, but deleting: a reaction on one is kept and shown as on a message, by the
/// server's id it has as any `PRIVMSG` does. Copying its text needs no field: every line
/// offers that.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an action of its own"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineActions {
    /// Reacting to it: the hover bar's emoji, and the picker for the rest.
    pub react: bool,
    /// Replying to it, which quotes it.
    pub reply: bool,
    /// Copying its own link, which opens it in every app, where its conversation has links.
    pub copy_link: bool,
    /// Correcting it ([`Chat::say_edit`]): only our own message or action, for
    /// [`super::EDIT_WINDOW`] after it was said.
    pub edit: bool,
    /// Deleting it for everyone (an IRCv3 `REDACT`): only our own.
    pub delete: bool,
    /// Showing what's attached to it ([`Chat::details`]): any line with the server's id.
    pub details: bool,
}

/// One item of a message's menu ([`Chat::message_menu`]), in the order every app lists them (the user's choice, 20 Nov
/// 2026): Reply, Edit, Copy text, its sealed parts, Copy code, Copy link, Copy link to message,
/// Details, React…, Delete…, Report….
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum MenuItem {
    /// Replying to it.
    Reply,
    /// Correcting it.
    Edit,
    /// Copying its words.
    CopyText,
    /// Copying each sealed part this device can open, which the app knows: none, one or more,
    /// each as [`super::copy_sealed_label`] says.
    CopySealed,
    /// Copying one of its code blocks, [`MenuEntry::code`].
    CopyCode,
    /// Copying the link in it, [`Line::link`].
    CopyLink,
    /// Copying its own link, where the app has the web address to make one.
    CopyMessageLink,
    /// What's attached to it ([`super::MESSAGE_DETAILS`]).
    Details,
    /// The emoji to give it.
    React,
    /// Deleting it for everyone, after asking.
    Delete,
    /// Reporting it, signed in to zirc.dev.
    Report,
}

/// An item of a message's menu as every app lists it ([`Chat::message_menu`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MenuEntry {
    /// What it does.
    pub item: MenuItem,
    /// What it says: `Copy code 2`, `Delete…`; empty for [`MenuItem::CopySealed`], whose parts
    /// say their own.
    pub label: String,
    /// For [`MenuItem::CopyCode`], the code it copies.
    pub code: Option<String>,
}

impl Chat {
    /// `line`'s menu, in the order and words every app lists it, as its [`LineActions`] allow,
    /// with Report where it may be reported ([`Chat::can_report`]): made when a menu opens, as
    /// one line's at a time, never for every line laid out.
    #[must_use]
    pub fn message_menu(&self, line: &Line) -> Vec<MenuEntry> {
        let reportable = line.id.as_deref().is_some_and(|id| self.can_report(id));
        menu(line, reportable)
    }
}

/// `line`'s menu, in order, as its [`LineActions`] allow, with Report where it may be
/// `reportable`.
fn menu(line: &Line, reportable: bool) -> Vec<MenuEntry> {
    let entry = |item, label: &str| MenuEntry {
        item,
        label: label.to_owned(),
        code: None,
    };
    let actions = &line.actions;
    let named = line.id.is_some();
    let mut menu = Vec::new();
    if actions.reply {
        menu.push(entry(MenuItem::Reply, super::REPLY_LABEL));
    }
    if actions.edit {
        menu.push(entry(MenuItem::Edit, super::EDIT_LABEL));
    }
    menu.push(entry(MenuItem::CopyText, super::COPY_TEXT));
    if named {
        menu.push(entry(MenuItem::CopySealed, ""));
    }
    let mut codes = Vec::new();
    codes_in(&line.rich, &mut codes);
    let of = codes.len();
    menu.extend(codes.into_iter().enumerate().map(|(at, code)| MenuEntry {
        item: MenuItem::CopyCode,
        label: copy_code_label(at + 1, of),
        code: Some(code),
    }));
    if line.link.is_some() {
        menu.push(entry(MenuItem::CopyLink, super::COPY_LINK));
    }
    if actions.copy_link {
        menu.push(entry(MenuItem::CopyMessageLink, super::COPY_MESSAGE_LINK));
    }
    if actions.details {
        menu.push(entry(MenuItem::Details, super::MESSAGE_DETAILS));
    }
    if actions.react {
        menu.push(entry(MenuItem::React, super::labels::REACT_PROMPT));
    }
    if actions.delete {
        menu.push(entry(MenuItem::Delete, &format!("{}…", super::DELETE)));
    }
    if reportable {
        menu.push(entry(
            MenuItem::Report,
            &format!("{}…", zirc_proto::report::words().report_message),
        ));
    }
    menu
}

/// Copy code's label for the `n`th of `of` code blocks in a message: `Copy code` for its only
/// one, `Copy code 2` for the second of several.
#[must_use]
pub fn copy_code_label(n: usize, of: usize) -> String {
    if of > 1 {
        format!("{} {n}", super::COPY_CODE)
    } else {
        super::COPY_CODE.to_owned()
    }
}

/// The code blocks in `blocks`, in order, those in quotes and lists included.
fn codes_in(blocks: &[RichBlock], out: &mut Vec<String>) {
    for block in blocks {
        match block {
            RichBlock::Code { text, .. } => out.push(text.clone()),
            RichBlock::Quote { blocks } => codes_in(blocks, out),
            RichBlock::Items { items, .. } => {
                for item in items {
                    codes_in(&item.blocks, out);
                }
            }
            _ => {}
        }
    }
}

impl LineActions {
    /// What a line of `kind` offers: with the server's `id`, reacting, replying and its link,
    /// unless it's a quiet line (people coming and going, a note); deleting too when it's
    /// `mine` and something said: a message, an action, a notice or a file, never an event,
    /// which its machine's zircd says, nor a card; correcting it when `editable`
    /// ([`super::editable`]).
    #[must_use]
    pub fn of(kind: LineKind, id: Option<&str>, mine: bool, editable: bool) -> Self {
        let named = id.is_some() && !matches!(kind, LineKind::Membership | LineKind::Note);
        let said = matches!(
            kind,
            LineKind::Chat | LineKind::Action | LineKind::Notice | LineKind::File
        );
        Self {
            react: named,
            reply: named,
            copy_link: named,
            edit: named && editable,
            delete: named && said && mine,
            details: id.is_some(),
        }
    }
}

/// One emoji given to a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineReaction {
    /// The emoji.
    pub emoji: String,
    /// How many gave it.
    pub count: u32,
    /// Whether we did.
    pub mine: bool,
    /// Who gave it, as hovering over its chip and a screen reader say it
    /// ([`super::reacted_by_label`]): `👍 by bob, ann`.
    pub label: String,
}

/// What a reply answers, shown above it as `↪ bob: first line…` and jumped to by its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineQuote {
    /// The answered message's id, as its [`Line::key`].
    pub id: String,
    /// Who said it, as their messages' heads name them. Empty when the message is no longer
    /// among those kept, which apps show as "a reply to an earlier message".
    pub from: String,
    /// Its first line, cut short; empty as `from` is.
    pub text: String,
    /// Who said it, as their messages' heads show them; `None` as `from` is empty.
    pub face: Option<Face>,
}

impl LineQuote {
    /// The quote as one line above the reply, for an app that draws it as text: `↪ bob:
    /// first line…`, or `↪ a reply to an earlier message` once that's no longer kept.
    #[must_use]
    pub fn line(&self) -> String {
        if self.from.is_empty() {
            format!("{} {}", super::QUOTE_MARK, super::EARLIER_REPLY)
        } else {
            format!("{} {}: {}", super::QUOTE_MARK, self.from, self.text)
        }
    }
}

/// Above the composer while a reply's written, as every app draws it: [`Self::lead`], a
/// space, who said what it answers in bold, then [`Self::rest`]: `Replying to bob: first
/// line…`, or `Replying to an earlier message` once that's no longer kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ReplyBar {
    /// `Replying to` ([`super::REPLYING_TO`]).
    pub lead: String,
    /// Who said what it answers, bold; none once that's no longer kept.
    pub from: Option<String>,
    /// After `from`: `: ` and its first line, cut short; [`super::EARLIER_MESSAGE`] without
    /// `from`.
    pub rest: String,
}

impl ReplyBar {
    /// The bar for a reply to `quote`, as [`Chat::quoted`] gives it: none, or one with no
    /// `from`, once it's no longer kept.
    #[must_use]
    pub fn of(quote: Option<LineQuote>) -> Self {
        let lead = super::REPLYING_TO.to_owned();
        match quote.filter(|quote| !quote.from.is_empty()) {
            Some(quote) => Self {
                lead,
                from: Some(quote.from),
                rest: format!(": {}", quote.text),
            },
            None => Self {
                lead,
                from: None,
                rest: super::EARLIER_MESSAGE.to_owned(),
            },
        }
    }

    /// What follows [`Self::lead`], for an app that draws it plain: `bob: first line…`.
    #[must_use]
    pub fn said(&self) -> String {
        format!("{}{}", self.from.as_deref().unwrap_or_default(), self.rest)
    }

    /// The bar as one line of text: `Replying to bob: first line…`.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{} {}", self.lead, self.said())
    }
}

/// A long message's one control ([`super::More`]): a text button on its own line under the
/// cut, or in a cut code block's footer beside Copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineMore {
    /// What it says: `Show 34 more lines`, or `Show less` while open.
    pub label: String,
    /// What a screen reader says: `Show 34 more lines of Maya Chen's agent's message`.
    pub spoken: String,
    /// How many lines the cut hides, code included.
    pub hidden: u32,
    /// Whether the message shows all of itself; [`Chat::toggle_more`] takes it.
    pub open: bool,
    /// While cut inside a code block, the last of [`Line::rich`]: how many of its first
    /// lines show, the last fading into the code's ground. `None` when the cut is in its
    /// words, or it's open.
    pub code_lines: Option<u32>,
}

impl LineMore {
    /// The row the terminal client draws for it, dim: `… 34 more lines · Enter shows them`
    /// ([`super::more::terminal_line`]).
    #[must_use]
    pub fn terminal_line(&self) -> String {
        super::more::terminal_line(
            self.open,
            usize::try_from(self.hidden).unwrap_or(usize::MAX),
        )
    }
}

/// An agent session, by what it goes by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionName {
    /// Its id, `cc-598aa1`, shown on hover or a long press.
    pub id: String,
    /// Its name, `zirc-chat`, as [`Chat::session_label`] says, or else its id.
    pub label: String,
}

/// A system line in parts ([`Parts`]): headed as a message is, by who, whose agent and the
/// time, never its kind; then its lead (marker and verb), its details and the paths under
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SystemLine {
    /// Its icon.
    pub icon: Icon,
    /// What to draw for it, one character wide ([`Icon::glyph`]).
    pub glyph: String,
    /// What its body starts with ([`Parts::lead`]): its marker and what was done, `✎ is
    /// about to change`; the marker alone when nothing was, and nothing for a post. Apps
    /// draw it, when it's there, as [`SystemLine::glyph`] in [`SystemLine::marker_tone`],
    /// then [`SystemLine::verb`] with its links, muted: never by cutting this up.
    pub lead: String,
    /// Who, as people read it: an agent session's name, a machine itself as `dana's agent`,
    /// a person's nick. Never a device.
    pub actor: String,
    /// The agent session it's from, or a landing's, shown dim by its name, its id on hover.
    pub session: Option<SessionName>,
    /// For a post to an agent session, that session, which [`Line::rich`] starts with as
    /// `→ zirc-chat`.
    pub to: Option<SessionName>,
    /// What they did, in a few words: `is about to change`; empty for a post.
    pub verb: String,
    /// What it was: an intent's summary, a commit's subject, a post's words.
    pub details: String,
    /// The paths it's about, set in monospace.
    pub paths: Vec<String>,
    /// The theme role its marker, the start of [`SystemLine::lead`], is drawn in
    /// ([`Icon::tone`]): `warning` for a conflict or a file two are changing. The rest of the
    /// lead is `muted`. Apps look it up; they never decide it from the icon.
    pub marker_tone: String,
    /// The theme role its details are drawn in ([`Icon::words_tone`]): `muted` for an
    /// agent's routine progress, `body` for the rest.
    pub details_tone: String,
    /// Whether its paths are listed under its words, one a line, the first
    /// [`SHOWN_PATHS`](super::SHOWN_PATHS) and a "Show N more"
    /// ([`show_more_paths_label`](super::show_more_paths_label)) for the rest.
    pub paths_listed: bool,
    /// Whether its details start a line of their own under its lead rather than following
    /// it on the same line: when its paths are listed under them, or they have lines of
    /// their own, as a push's commits do.
    pub details_apart: bool,
    /// [`SystemLine::verb`] with its links ([`Chat::linked`]): a push's branch and commit to
    /// their pages on the forge. Empty when none of it links.
    pub verb_links: Vec<RichSpan>,
    /// [`SystemLine::details`] with its links. Empty when none of it links.
    pub details_links: Vec<RichSpan>,
    /// What its row draws under its head, when it's a [`RowShape::System`] row.
    pub body: SystemBody,
    /// The event as one muted line, for an app that draws an agent's routine work quietly
    /// ([`Parts::brief`](super::Parts::brief)): `checkout is about to change Coupon.tsx`.
    /// `None` for what keeps its full form: what someone has reacted to, and everything in
    /// an agent's own conversation ([`Chat::agent_lines`]). Apps draw it as it says.
    pub brief: Option<String>,
    /// When the last of a fold happened, on the view's clock, when it's later than the
    /// first: a commit and its push, made one line.
    pub until_clock: Option<String>,
    /// How a landing went, as an icon before its words; `None` for anything else.
    pub outcome: Option<Icon>,
    /// Whether it's a landing's card: its branch, build and stages on one line, which
    /// [`Line::rich`] draws, updated in place by later events.
    pub card: bool,
    /// A card's buttons, those it shows us ([`Chat::shows_action`]), when it has any.
    pub buttons: Option<CardButtons>,
    /// What tells this version of a card apart from the next ([`Landing::card_version`]),
    /// so what a press came to shows until the integration posts the card again; `None`
    /// with no card.
    pub card_version: Option<String>,
    /// For a card's update said further down ([`super::Pointer`]): the card's line's key,
    /// which apps bring into view when the line is clicked or tapped, as a reply's quote
    /// does ([`LineQuote::id`]).
    pub points_to: Option<String>,
}

impl SystemLine {
    /// What its body says under its lead, its line's `text` being [`Line::text`]: its
    /// details, or, for an event zirc-chat has no verb or details for (zircd's lease and
    /// rename lines, say), its own words, rather than a lead over nothing ([`Parts::words`]).
    #[must_use]
    pub fn words<'a>(&'a self, text: &'a str) -> &'a str {
        if self.verb.is_empty() && self.details.is_empty() {
            text
        } else {
            &self.details
        }
    }
}

/// A card's buttons ([`zirc_proto::card`]): which card, by its sender and id, and the
/// actions it shows us, which press through zirc.dev.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CardButtons {
    /// The nick that posted the card, whose integration a press reaches.
    pub sender: String,
    /// The card's id.
    pub card: String,
    /// Tells its versions apart, so what a press came to shows until the card changes: the
    /// same as [`SystemLine::card_version`].
    pub key: String,
    /// Its buttons, in order.
    pub actions: Vec<CardAction>,
}

/// One of a card's buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CardAction {
    /// The integration's own name for it.
    pub id: String,
    /// What the button says.
    pub label: String,
    /// What to ask before pressing it, if anything; the ask's other choice is
    /// [`super::CANCEL`] in every app (the user's choice, 20 Nov 2026).
    pub confirm: Option<String>,
    /// Whether it's the card's first, drawn tonal; the rest are outlined, in every app (the
    /// user's choice, 20 Nov 2026).
    pub primary: bool,
}

/// How a shared file is drawn ([`file::shape`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum FileShape {
    /// Inline, resized by zirc.dev.
    Image,
    /// A poster and a player.
    Video,
    /// A card with the start of its text, which zirc.dev previews.
    Text,
    /// A card with its name, size and a download button.
    Other,
}

impl From<file::Shape> for FileShape {
    fn from(shape: file::Shape) -> Self {
        match shape {
            file::Shape::Image => Self::Image,
            file::Shape::Video => Self::Video,
            file::Shape::Text => Self::Text,
            file::Shape::Other => Self::Other,
        }
    }
}

/// A file someone shared, hosted by the web app, as every app draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineFile {
    /// Its name.
    pub name: String,
    /// Its media type, `image/jpeg`.
    pub mime: String,
    /// Its size in bytes.
    pub size: u64,
    /// That as people read it: `2.3 MB` ([`file::size_text`]).
    pub size_text: String,
    /// How it's drawn.
    pub shape: FileShape,
    /// Whether it's a GIF, which may move: played [`ANIMATION_PLAYS`] times, then
    /// rested on its last frame ([`file::animated`]).
    pub animated: bool,
    /// Where it's downloaded or played: zirc.dev sends a signed-in browser on to it.
    pub url: Option<String>,
    /// The width to draw an image or a video's poster at, in points, fitted to
    /// [`MEDIA_WIDTH`] by [`MEDIA_HEIGHT`], when its size is known.
    pub width: Option<u32>,
    /// The height to draw it at, as `width`.
    pub height: Option<u32>,
    /// An image's, or a video's poster's, sizes zirc.dev resizes it to, as an `<img>`'s
    /// `srcset`: `<url>?w=64 64w, …`.
    pub srcset: Option<String>,
    /// A video's poster, sharp on a screen of two pixels a point.
    pub poster: Option<String>,
}

impl Chat {
    /// The conversation `name`'s lines, oldest first, as every app shows it, times on a
    /// clock `offset` from UTC; with `people_only`, only what people said and what mentions
    /// us, as a conversation's "People only" filter shows it ([`super::people_only`]).
    /// Empty for a conversation there isn't.
    #[must_use]
    pub fn lines(&self, name: &str, offset: FixedOffset, people_only: bool) -> Vec<Line> {
        let Some(conversation) = self.conversation(name) else {
            return Vec::new();
        };
        let mut lines = Layout::new(self, conversation, offset, HashMap::new()).lines(people_only);
        settle_briefs(&mut lines);
        lines
    }

    /// The agent session `name`'s conversation (`<machine>/<session>`) as its lines: its
    /// activity in every room we're in and what we and it said to each other
    /// ([`Chat::agent_activity`]), each line of activity with its room. As its own: its
    /// lines leave out whose agent it is, as the header says it, and a line from another
    /// room than the one before starts a new run, with its own head and chip, so each chip
    /// is true for its run. Empty for any other conversation.
    #[must_use]
    pub fn agent_lines(&self, name: &str, offset: FixedOffset) -> Vec<Line> {
        let Some((_, session)) = agent(name) else {
            return Vec::new();
        };
        let activity = self.agent_activity(name);
        let conversation = activity_conversation(name, &activity);
        let rooms = conversation
            .entries
            .iter()
            .zip(activity)
            .filter_map(|(entry, line)| Some((std::ptr::from_ref(entry), line.room?)))
            .collect();
        let mut lines = Layout::new(self, &conversation, offset, rooms).lines(false);
        let mut last: Option<String> = None;
        for line in &mut lines {
            if line.day.is_some() {
                last = None;
            }
            let Some(speaker) = &mut line.speaker else {
                continue;
            };
            if speaker.session.as_deref() == Some(session) {
                speaker.detail = None;
            }
            if line.continues && line.room != last {
                line.continues = false;
            }
            last.clone_from(&line.room);
        }
        lines
    }
}

/// Heads again a line carrying on a run from a brief one ([`Conversation::brief`]), as a
/// brief line has no avatar or name for it to carry on from, so no app decides it.
fn settle_briefs(lines: &mut [Line]) {
    let mut after_brief = false;
    for line in lines {
        let brief = line
            .system
            .as_ref()
            .is_some_and(|system| system.brief.is_some());
        if after_brief && !brief {
            line.continues = false;
        }
        after_brief = brief;
    }
}

/// Puts a conversation's items into lines.
struct Layout<'a> {
    chat: &'a Chat,
    conversation: &'a Conversation,
    offset: FixedOffset,
    /// The room each line of an agent's activity was said in, by its entry.
    rooms: HashMap<*const Entry, String>,
    keys: Keys,
}

impl<'a> Layout<'a> {
    fn new(
        chat: &'a Chat,
        conversation: &'a Conversation,
        offset: FixedOffset,
        rooms: HashMap<*const Entry, String>,
    ) -> Self {
        Self {
            chat,
            conversation,
            offset,
            rooms,
            keys: Keys::default(),
        }
    }

    fn lines(mut self, people_only: bool) -> Vec<Line> {
        let offset = self.offset;
        let items = self.chat.items(self.conversation, |time| {
            time.with_timezone(&offset).date_naive()
        });
        let items = if people_only {
            super::people_only(items)
        } else {
            items
        };
        let now = (self.chat.clock)();
        let today = now.with_timezone(&offset).date_naive();
        let mut lines = Vec::with_capacity(items.len());
        let mut day = None;
        for item in items {
            if let Item::Day(date) = item {
                day = Some(super::day_label(date, today));
                continue;
            }
            let editable = item
                .said_entry()
                .is_some_and(|entry| super::editable(entry, self.chat.me(), now));
            let mut line = self.line(item);
            line.day = day.take();
            line.actions = LineActions::of(line.kind, line.id.as_deref(), line.mine, editable);
            line.shape = RowShape::of(&line);
            lines.push(line);
        }
        lines
    }

    /// `item`'s line; never a day, which is the divider on the line after it.
    fn line(&mut self, item: Item<'_>) -> Line {
        match item {
            Item::Day(_) => unreachable!("a day goes on the line after it"),
            Item::Message {
                entry,
                text,
                continues,
            } => {
                let mut line = self.said(entry, LineKind::Chat, text, continues);
                self.body(
                    &mut line,
                    entry.rich(),
                    &entry.place(&self.conversation.name),
                );
                self.sealed(&mut line, entry);
                line
            }
            Item::Notice {
                entry,
                text,
                continues,
            } => {
                let mut line = self.said(entry, LineKind::Notice, text, continues);
                self.body(
                    &mut line,
                    entry.rich(),
                    &entry.place(&self.conversation.name),
                );
                line
            }
            Item::Action {
                entry,
                text,
                continues,
            } => self.said(entry, LineKind::Action, text, continues),
            Item::Event {
                entry,
                text,
                parts,
                continues,
            } => {
                let mut line = self.said(entry, LineKind::Event, &text, continues);
                let mut system = self.system(&parts, parts.session.clone(), line.id.as_deref());
                // A post's words read as a message's: drawn, and cut when long, as one is,
                // after whom it's to.
                if parts.icon == Icon::Post && !parts.details.is_empty() {
                    let place = entry.place(&self.conversation.name);
                    self.post(&mut line, &parts.details, system.to.as_ref(), &place);
                    system.body = SystemBody::Blocks;
                }
                line.system = Some(system);
                line
            }
            Item::Landing(landing) => self.landing(&landing),
            Item::Pointer(pointer) => {
                let parts = pointer.parts();
                let mut line = self.said(pointer.entry, LineKind::Event, &pointer.text(), false);
                line.system = Some(SystemLine {
                    // Its key is its id: one without, which IRC always gives, has none to jump to.
                    points_to: pointer.card.id.clone(),
                    ..self.system(&parts, None, line.id.as_deref())
                });
                line
            }
            Item::File {
                entry,
                file,
                from,
                continues,
            } => self.file(entry, file, &from, continues),
            Item::Note { entry, text } => {
                let key = self.keys.of(entry);
                let mut line = self.blank(key, Some(entry), entry.time, LineKind::Note);
                line.text = text;
                line
            }
            Item::Membership(membership) => {
                let key = self.keys.next(membership.time.timestamp_millis());
                let mut line = self.blank(key, None, membership.time, LineKind::Membership);
                line.text = membership.text();
                line.system = Some(self.system(&membership.parts(), None, None));
                line
            }
        }
    }

    /// A line with only its key, id, room, time and kind.
    fn blank(
        &self,
        key: String,
        entry: Option<&Entry>,
        time: DateTime<Utc>,
        kind: LineKind,
    ) -> Line {
        Line {
            key,
            id: entry.and_then(|entry| entry.id.clone()),
            room: entry.and_then(|entry| self.rooms.get(&std::ptr::from_ref(entry)).cloned()),
            time_ms: time.timestamp_millis(),
            clock: self.clock(time),
            day: None,
            kind,
            shape: RowShape::Message,
            continues: false,
            from: entry.map(|entry| entry.from.clone()).unwrap_or_default(),
            speaker: None,
            presence: None,
            guest: false,
            text: String::new(),
            rich: Vec::new(),
            link: None,
            mention: false,
            mine: false,
            edited: false,
            reactions: Vec::new(),
            quote: None,
            more: None,
            system: None,
            file: None,
            actions: LineActions::default(),
        }
    }

    /// What someone said or did, `entry`, headed by them: who, their face and dot, whether
    /// it names us or is ours, its reactions and what it answers.
    fn said(&mut self, entry: &Entry, kind: LineKind, text: &str, continues: bool) -> Line {
        let key = self.keys.of(entry);
        let nick = entry.from.as_str();
        let me = self.chat.me();
        Line {
            continues,
            speaker: (!nick.is_empty()).then(|| self.chat.line_speaker(entry)),
            presence: (!nick.is_empty()).then(|| self.presence(nick)),
            guest: self.chat.is_guest(nick),
            text: text.to_owned(),
            link: first_link(text).map(str::to_owned),
            mention: self.conversation.highlights(entry),
            mine: !me.is_empty() && fold(nick) == fold(me),
            edited: entry.edited,
            reactions: self.reactions(entry),
            quote: self.quote(entry),
            ..self.blank(key, Some(entry), entry.time, kind)
        }
    }

    /// `line`, `entry`'s, with its sealed parts as chips: in spans though its words are
    /// plain, and its text, which apps copy, as [`Chat::sealed_text`] says it.
    fn sealed(&self, line: &mut Line, entry: &Entry) {
        if entry.sealed.is_none() {
            return;
        }
        line.text = self.chat.sealed_text(entry, &line.text);
        if line.rich.is_empty() {
            line.rich = self.chat.flat_in(
                &entry.place(&self.conversation.name),
                self.chat.body(&line.key, entry.rich(), line.mention).blocks,
            );
        }
        self.chat.seal_spans(entry, &mut line.rich);
    }

    /// `line`'s words as `blocks`, drawn and cut when long, with its control: none when
    /// they're one plain paragraph shown whole, which its text says. Its files and commits
    /// link as said in `place` ([`Entry::place`]).
    fn body(&self, line: &mut Line, blocks: Vec<Block>, place: &str) {
        self.words(line, blocks, false, place);
    }

    /// [`Self::body`], or, `always`, as blocks though they're plain.
    fn words(&self, line: &mut Line, blocks: Vec<Block>, always: bool, place: &str) {
        let shown = self.chat.body(&line.key, blocks, line.mention);
        let more = shown.more.map(|more| {
            // Whose message, as its head says: `Maya Chen's agent`, or the person's name.
            let whose = line
                .speaker
                .as_ref()
                .map(|speaker| speaker.detail.clone().unwrap_or(speaker.name.clone()))
                .unwrap_or_default();
            LineMore {
                label: more.label(),
                spoken: more.spoken(&whose),
                hidden: count(more.hidden),
                open: more.open,
                code_lines: more.code_lines.map(count),
            }
        });
        let plain = more.is_none() && is_plain(&shown.blocks);
        let rich = self.chat.flat_in(place, shown.blocks);
        // Plain words draw as text, unless they name code that links.
        if !always && plain && !names_code(&rich) {
            return;
        }
        line.rich = rich;
        line.more = more;
    }

    /// `line`'s words as a post's, `details`: always as blocks, drawn and cut when long as a
    /// message's are, after `→ zirc-chat ` when it's `to` an agent session.
    fn post(&self, line: &mut Line, details: &str, to: Option<&SessionName>, place: &str) {
        let mut blocks = super::rich(details);
        if let Some(to) = to {
            let span = |text: &str, kind| Span {
                text: text.to_owned(),
                kind,
                ..Span::default()
            };
            let lead = [
                span("→ ", SpanKind::Text),
                span(&to.label, SpanKind::Session(to.id.clone())),
                span(" ", SpanKind::Text),
            ];
            match blocks.first_mut() {
                Some(Block::Paragraph(spans)) => {
                    spans.splice(0..0, lead);
                }
                _ => blocks.insert(0, Block::Paragraph(lead.into())),
            }
        }
        self.words(line, blocks, true, place);
    }

    /// A system line from `parts`, about the agent session `session`, if any, said as the
    /// message `id`.
    fn system(&self, parts: &Parts, session: Option<String>, id: Option<&str>) -> SystemLine {
        let lead = parts.lead();
        let paths_listed = !parts.paths.is_empty();
        let details_apart = !lead.is_empty()
            && !parts.details.is_empty()
            && (paths_listed || parts.details.contains('\n'));
        SystemLine {
            marker_tone: parts.icon.tone().to_owned(),
            details_tone: parts.icon.words_tone().to_owned(),
            paths_listed,
            details_apart,
            icon: parts.icon,
            glyph: parts.icon.glyph().to_owned(),
            lead,
            actor: parts.actor.clone(),
            session: session.map(|id| self.session(id)),
            to: parts.to.clone().map(|id| self.session(id)),
            verb_links: self.chat.linked(&parts.verb, &parts.links),
            details_links: self.chat.linked(&parts.details, &parts.links),
            verb: parts.verb.clone(),
            details: parts.details.clone(),
            paths: parts.paths.clone(),
            body: SystemBody::Parts,
            brief: self.conversation.brief(parts, id),
            until_clock: (parts.until != parts.time).then(|| self.clock(parts.until)),
            outcome: None,
            card: false,
            buttons: None,
            card_version: None,
            points_to: None,
        }
    }

    /// How a landing went, or any card, as CI or an integration posts it: one line, its
    /// words the card's line or what was said, with its branch and commit linked, and its
    /// outcome and buttons.
    fn landing(&mut self, landing: &Landing<'_>) -> Line {
        let entry = landing.entry;
        let parts = landing.parts();
        let card_line = landing.card_line();
        // For copying: the card's line as it reads, or what CI said.
        let text = card_line.as_ref().map_or_else(
            || landing.said().to_owned(),
            |line| line.iter().map(|span| span.text.as_str()).collect(),
        );
        let mut line = self.said(entry, LineKind::Landing, &text, landing.continues);
        // Its updates under it, the earlier ones folded away while there are many, and a
        // done card's steps on one line, until the person opens them.
        let more = self
            .chat
            .trail_more(&line.key, landing.updates.len(), landing.folds_steps());
        let steps = more.is_none_or(|more| more.open || more.steps.is_none());
        let blocks = link_blocks(landing.rich_steps(steps), &landing.links());
        let clock = |at| self.clock(at);
        let trail = super::trail_stubs(landing.trail(more.as_ref()), &clock);
        line.rich = self.chat.flat(self.chat.drawn(blocks));
        line.rich.extend(self.chat.flat_trail(trail));
        line.more = more.map(|more| {
            let whose = line
                .speaker
                .as_ref()
                .map(|speaker| speaker.detail.clone().unwrap_or(speaker.name.clone()))
                .unwrap_or_default();
            LineMore {
                label: more.label(),
                spoken: more.spoken(&whose),
                hidden: count(more.hidden),
                open: more.open,
                code_lines: None,
            }
        });
        line.link = landing
            .page
            .clone()
            .or_else(|| landing.url.map(str::to_owned));
        let outcome = (parts.icon != Icon::Post).then_some(parts.icon);
        let version = landing.card_version();
        // Its card's line, or an older one's words, as a message's are; its parts when it
        // has neither.
        let body = match (card_line.is_some(), line.rich.is_empty()) {
            (_, true) => SystemBody::Parts,
            (true, false) => SystemBody::Card,
            (false, false) => SystemBody::Blocks,
        };
        // A card's picture, under its title, as a shared image is drawn: its alt as its name.
        line.file = landing
            .card
            .as_ref()
            .and_then(|card| card.image.as_ref())
            .and_then(|image| {
                self.line_file(&image.file).map(|drawn| LineFile {
                    name: image.alt.clone(),
                    ..drawn
                })
            });
        // The session it's about, dim after it, unless its card's line names it already, as
        // a branch called after its session does: `checkout · build 418 · landed`, then
        // `checkout` again (the user's choice, 19 Nov 2026).
        let session = landing
            .session
            .map(str::to_owned)
            .filter(|id| card_line.is_none() || !names_word(&text, &self.chat.session_label(id)));
        line.system = Some(SystemLine {
            outcome,
            body,
            card: card_line.is_some(),
            buttons: self.buttons(landing, &version),
            card_version: landing.card.as_ref().map(|_| version),
            ..self.system(&parts, session, line.id.as_deref())
        });
        line
    }

    /// A card's buttons, those it shows us, when it has any.
    fn buttons(&self, landing: &Landing<'_>, version: &str) -> Option<CardButtons> {
        let card = landing.card.as_ref()?;
        let actions: Vec<CardAction> = card
            .actions
            .iter()
            .filter(|action| self.chat.shows_action(card, action))
            .enumerate()
            .map(|(at, action)| CardAction {
                id: action.id.clone(),
                label: action.label.clone(),
                confirm: action.confirm.clone(),
                primary: at == 0,
            })
            .collect();
        (!actions.is_empty()).then(|| CardButtons {
            sender: landing.entry.from.clone(),
            card: card.id.clone(),
            key: version.to_owned(),
            actions,
        })
    }

    /// A shared file: drawn when the web app hosts it, otherwise only said, as a note,
    /// loading nothing. `from` is who shared it, as plain text names them.
    fn file(
        &mut self,
        entry: &Entry,
        file: &zirc_proto::File,
        from: &str,
        continues: bool,
    ) -> Line {
        let Some(drawn) = self.line_file(file) else {
            let key = self.keys.of(entry);
            let mut line = self.blank(key, Some(entry), entry.time, LineKind::Note);
            line.text = format!(
                "{from} shared {} ({}) from elsewhere, not shown",
                file.name,
                file::size_text(file.size)
            );
            return line;
        };
        let mut line = self.said(entry, LineKind::File, &file.name, continues);
        // Its address, for every app's Copy link: its name isn't one.
        line.link = Some(file.url.clone());
        line.file = Some(drawn);
        line
    }

    /// `file` as apps draw it, when the web app hosts it: never another address, as for a
    /// shared file and a card's image alike.
    fn line_file(&self, file: &zirc_proto::File) -> Option<LineFile> {
        let hosted = self
            .chat
            .files_web()
            .is_some_and(|web| file::is_hosted(file, web));
        if !hosted {
            return None;
        }
        let shape = FileShape::from(file::shape(file));
        let drawn = matches!(shape, FileShape::Image | FileShape::Video);
        let fitted =
            file::fit(file.width, file.height, MEDIA_WIDTH, MEDIA_HEIGHT).filter(|_| drawn);
        Some(LineFile {
            name: file.name.clone(),
            mime: file.mime.clone(),
            size: file.size,
            size_text: file::size_text(file.size),
            shape,
            animated: file::animated(file),
            url: Some(file.url.clone()),
            width: fitted.map(|(width, _)| width),
            height: fitted.map(|(_, height)| height),
            srcset: drawn.then(|| srcset(file)),
            poster: (shape == FileShape::Video).then(|| file::image_url(file, MEDIA_WIDTH * 2)),
        })
    }

    /// Whether `nick` is around, as the dot on their face says: here in the conversation,
    /// away or gone ([`Standing::of`]).
    fn presence(&self, nick: &str) -> Presence {
        let here = self.conversation.members.contains_key(&fold(nick));
        Standing::of(here, self.chat.away(nick)).presence()
    }

    fn reactions(&self, entry: &Entry) -> Vec<LineReaction> {
        let Some(id) = entry.id.as_deref() else {
            return Vec::new();
        };
        let me = self.chat.me();
        self.conversation
            .reactions(id)
            .iter()
            .map(|reaction| LineReaction {
                emoji: reaction.emoji.clone(),
                count: count(reaction.nicks.len()),
                mine: reaction.by(me),
                label: super::reacted_by_label(&reaction.emoji, &reaction.nicks),
            })
            .collect()
    }

    /// What `entry` answers, when it's a reply: who said it and its first line, named and
    /// drawn as their messages are; only its id when it's no longer among those kept.
    fn quote(&self, entry: &Entry) -> Option<LineQuote> {
        let id = entry.reply_to.as_ref()?;
        Some(
            self.chat
                .line_quote(self.conversation, entry)
                .unwrap_or_else(|| LineQuote {
                    id: id.clone(),
                    from: String::new(),
                    text: String::new(),
                    face: None,
                }),
        )
    }

    fn session(&self, id: String) -> SessionName {
        SessionName {
            label: self.chat.session_label(&id),
            id,
        }
    }

    /// `at` on the view's clock: `15:48`.
    fn clock(&self, at: DateTime<Utc>) -> String {
        super::clock(at.with_timezone(&self.offset).naive_local())
    }
}

/// Whether `text` has `word` as a word of its own, as a branch's name is in a card's line:
/// `checkout` in `checkout · build 418 · landed`, not in `checkout-v2`.
fn names_word(text: &str, word: &str) -> bool {
    text.split(|c: char| c.is_whitespace() || matches!(c, '·' | ':' | ',' | '(' | ')'))
        .any(|part| part == word)
}

/// Every width zirc.dev resizes `file`'s image to, for a view to pick from by its pixels.
fn srcset(file: &zirc_proto::File) -> String {
    file::WIDTHS
        .map(|width| format!("{} {width}w", file::image_url(file, width)))
        .join(", ")
}

/// `n` as records count, which no conversation reaches past.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Keys for lines: the server's id where there is one, otherwise the time and how many
/// id-less lines came before it at that time, which stay put as lines arrive after them.
#[derive(Default)]
pub(super) struct Keys {
    last: Option<i64>,
    same: u32,
}

impl Keys {
    pub(super) fn of(&mut self, entry: &Entry) -> String {
        entry
            .id
            .clone()
            .unwrap_or_else(|| self.next(entry.time.timestamp_millis()))
    }

    /// Moves past `entry` as [`Keys::of`] does, without making its key: for what passes
    /// over every entry to key only a few, as Activity does.
    pub(super) fn skip(&mut self, entry: &Entry) {
        if entry.id.is_none() {
            self.step(entry.time.timestamp_millis());
        }
    }

    fn next(&mut self, time_ms: i64) -> String {
        format!("{time_ms}:{}", self.step(time_ms))
    }

    /// How many before this one had the time `time_ms`, after the one before it.
    fn step(&mut self, time_ms: i64) -> u32 {
        if self.last == Some(time_ms) {
            self.same += 1;
        } else {
            self.last = Some(time_ms);
            self.same = 0;
        }
        self.same
    }
}

/// Whether `blocks` name a file or a commit that links ([`super::code_ref`]).
fn names_code(blocks: &[RichBlock]) -> bool {
    let mut names = false;
    RichBlock::each_span(blocks, &mut |span| names |= span.code.is_some());
    names
}

impl Chat {
    /// What the message `id` in `conversation` says, as the bar over the composer shows it
    /// while a reply to it is written (`Replying to bob: first line…`), and as a reply's
    /// [`Line::quote`] does: who, as their messages' heads name them, and its first line cut
    /// to [`super::QUOTE_CHARS`]. `None` once it's no longer among those kept.
    #[must_use]
    pub fn quoted(&self, conversation: &Conversation, id: &str) -> Option<LineQuote> {
        let asking = Entry {
            reply_to: Some(id.to_owned()),
            ..Entry::info((self.clock)(), String::new())
        };
        self.line_quote(conversation, &asking)
    }

    /// What `entry` answers in `conversation`, as a [`LineQuote`].
    fn line_quote(&self, conversation: &Conversation, entry: &Entry) -> Option<LineQuote> {
        self.quote(conversation, entry).map(|quote| LineQuote {
            face: Some(self.face(&quote.avatar)),
            id: quote.id,
            from: quote.name,
            text: quote.text,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use zirc_proto::card::{Action, Card, State, Who};
    use zirc_proto::{Event, Message};

    use super::*;
    use crate::chat::{Link, Options, RichSpanKind};

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn feed(chat: &mut Chat, lines: &[&str]) {
        for line in lines {
            let _ = chat.receive(line.parse().unwrap());
        }
    }

    /// A chat as `me`, connected and in `#api` with alice and bob, at noon on 30 October.
    fn chat() -> Chat {
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        chat.clock = noon;
        chat.connected(Link {
            nick: "me".into(),
            chathistory: true,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
            metadata: false,
        });
        feed(
            &mut chat,
            &[
                ":me!u@h JOIN #api",
                ":s 353 me = #api :me alice bob",
                ":s 366 me #api :End",
            ],
        );
        chat
    }

    /// `line` said at `time` on 30 October, with the id `id` when there is one.
    fn at(time: &str, id: Option<&str>, line: &str) -> String {
        let id = id.map(|id| format!(";msgid={id}")).unwrap_or_default();
        format!("@time=2026-10-30T{time}.000Z{id} {line}")
    }

    /// `message` from `nick` in `room`, as a line to feed.
    /// `me-mbp` is our machine, as its host says.
    fn zirc(nick: &str, room: &str, message: &Message) -> String {
        let mut line = crate::presence::line_from(nick, room, message).unwrap();
        if nick == "me-mbp" {
            line.source = Some(format!(
                "{nick}!u@{}",
                zirc_proto::names::machine_host("me")
            ));
        }
        line.to_string()
    }

    fn named(session: &str, name: &str, event: Event) -> Message {
        Message {
            name: Some(name.into()),
            ..Message::new(session, event)
        }
    }

    fn text(body: &str) -> Event {
        Event::Text { body: body.into() }
    }

    fn api(chat: &Chat) -> Vec<Line> {
        chat.lines("#api", utc(), false)
    }

    fn line<'a>(lines: &'a [Line], text: &str) -> &'a Line {
        lines
            .iter()
            .find(|line| line.text.contains(text))
            .unwrap_or_else(|| panic!("no line says {text}: {lines:#?}"))
    }

    #[test]
    fn plain_words_naming_code_link_once_the_forge_is_known() {
        let mut chat = chat();
        let said = ":alice!u@h PRIVMSG #api :broke at src/a.rs:6 in 3f2a1c0";
        feed(&mut chat, &[&at("09:00:00", Some("p1"), said)]);
        assert!(line(&api(&chat), "broke at").rich.is_empty());
        let push = named(
            "cc-1",
            "checkout",
            Event::Push {
                branch: "main".into(),
                head: "abc1234def".into(),
                forced: false,
                subjects: Vec::new(),
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: Some("https://github.com/acme/api".into()),
                on: None,
            },
        );
        feed(
            &mut chat,
            &[&at(
                "09:01:00",
                Some("p2"),
                &zirc("me-mbp", "#api/agents", &push),
            )],
        );
        let lines = api(&chat);
        let RichBlock::Paragraph { spans } = &line(&lines, "broke at").rich[0] else {
            panic!("a paragraph");
        };
        let linked: Vec<&str> = spans
            .iter()
            .filter(|span| span.code.is_some())
            .map(|span| span.text.as_str())
            .collect();
        assert_eq!(linked, ["src/a.rs:6", "3f2a1c0"]);
        // Words naming nothing stay plain.
        assert!(line(&lines, "broke at").text.contains("broke"));
    }

    #[test]
    fn keys_are_stable_and_unique_and_days_divide_once() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                "@time=2026-10-29T09:00:00.000Z;msgid=y1 :alice!u@h PRIVMSG #api :yesterday",
                &at("09:00:00", None, ":alice!u@h PRIVMSG #api :one"),
                &at("09:00:00", None, ":alice!u@h PRIVMSG #api :two"),
                &at("09:01:00", Some("m3"), ":bob!u@h PRIVMSG #api :three"),
                &at("09:02:00", None, ":bob!u@h PRIVMSG #api :four"),
            ],
        );
        let lines = api(&chat);
        let keys: Vec<&str> = lines.iter().map(|line| line.key.as_str()).collect();
        let nine = Utc
            .with_ymd_and_hms(2026, 10, 30, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        let two = Utc
            .with_ymd_and_hms(2026, 10, 30, 9, 2, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            keys,
            [
                "y1".to_owned(),
                format!("{nine}:0"),
                format!("{nine}:1"),
                "m3".to_owned(),
                format!("{two}:0"),
            ]
        );
        // The same lines, laid out again, keep their keys.
        let again: Vec<String> = api(&chat).into_iter().map(|line| line.key).collect();
        assert_eq!(again, keys);
        let days: Vec<Option<&str>> = lines.iter().map(|line| line.day.as_deref()).collect();
        assert_eq!(days, [Some("Yesterday"), Some("Today"), None, None, None]);
        assert_eq!(lines[1].clock, "09:00");
        assert_eq!(lines[1].time_ms, nine);
        // The first of a day starts a run; the rest of a sender's carry it on.
        let continues: Vec<bool> = lines.iter().map(|line| line.continues).collect();
        assert_eq!(continues, [false, false, true, false, true]);
        assert!(lines.iter().all(|line| line.kind == LineKind::Chat));
    }

    #[test]
    fn times_and_days_are_on_the_views_clock() {
        let mut chat = chat();
        feed(
            &mut chat,
            &["@time=2026-10-29T23:30:00.000Z :alice!u@h PRIVMSG #api :late"],
        );
        let lines = chat.lines("#api", FixedOffset::east_opt(3600).unwrap(), false);
        assert_eq!(lines[0].clock, "00:30");
        assert_eq!(lines[0].day.as_deref(), Some("Today"));
        assert_eq!(api(&chat)[0].day.as_deref(), Some("Yesterday"));
    }

    #[test]
    fn says_who_said_it_and_what_concerns_us() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                ":s 354 me 42 h bob G bob :Bob",
                ":guest-7k2q!u@h JOIN #api * :Guest",
                &at(
                    "09:00:00",
                    Some("m1"),
                    ":alice!u@h PRIVMSG #api :hey @me, look",
                ),
                &at("09:01:00", Some("m2"), ":ME!u@h PRIVMSG #api :on it")
                    .replace(";msgid=m2", ";msgid=m2;+zirc.dev/client=desktop"),
                &at("09:02:00", Some("m3"), ":bob!u@h PRIVMSG #api :fine"),
                &at(
                    "09:03:00",
                    Some("m4"),
                    ":guest-7k2q!u@h PRIVMSG #api :hello?",
                ),
                &at("09:04:00", Some("m5"), ":carol!u@h PRIVMSG #api :drive-by"),
            ],
        );
        let lines = api(&chat);
        let alice = line(&lines, "look");
        assert!(alice.mention && !alice.mine);
        assert_eq!(alice.from, "alice");
        assert_eq!(alice.presence, Some(Presence::Here));
        let speaker = alice.speaker.as_ref().unwrap();
        assert_eq!((speaker.name.as_str(), speaker.app), ("alice", false));
        // Ours by its folded nick, whatever its case; never the app it was sent from.
        let mine = line(&lines, "on it");
        assert!(mine.mine && !mine.mention);
        assert_eq!(line(&lines, "fine").presence, Some(Presence::Away));
        let guest = line(&lines, "hello?");
        assert!(guest.guest);
        assert_eq!(line(&lines, "drive-by").presence, Some(Presence::Gone));
        assert!(!line(&lines, "fine").guest);
    }

    #[test]
    fn plain_words_have_no_blocks_and_rich_ones_do() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("m1"),
                    ":alice!u@h PRIVMSG #api :just words",
                ),
                &at(
                    "09:01:00",
                    Some("m2"),
                    ":alice!u@h PRIVMSG #api :**bold** see https://example.com/x",
                ),
                &at("09:02:00", Some("m3"), ":alice!u@h NOTICE #api :a *notice*"),
                &at(
                    "09:03:00",
                    Some("m4"),
                    ":alice!u@h PRIVMSG #api :\x01ACTION waves\x01",
                ),
            ],
        );
        let lines = api(&chat);
        let plain = line(&lines, "just words");
        assert!(plain.rich.is_empty() && plain.more.is_none());
        let rich = line(&lines, "bold");
        assert_eq!(rich.text, "**bold** see https://example.com/x");
        assert_eq!(rich.link.as_deref(), Some("https://example.com/x"));
        let [RichBlock::Paragraph { spans }] = rich.rich.as_slice() else {
            panic!("one paragraph: {:?}", rich.rich);
        };
        assert!(spans[0].bold);
        assert!(
            spans
                .iter()
                .any(|span| matches!(&span.kind, RichSpanKind::Link { url } if url == "https://example.com/x"))
        );
        // A notice has its speaker and rich text as a message does.
        let notice = line(&lines, "notice");
        assert_eq!(notice.kind, LineKind::Notice);
        assert!(notice.speaker.is_some());
        assert!(!notice.rich.is_empty());
        let action = line(&lines, "waves");
        assert_eq!(action.kind, LineKind::Action);
        assert_eq!(action.text, "waves");
    }

    /// What `nick` said or did, `kind`, in `#api` at `time` on 30 October, as an entry put
    /// in directly: for text of several lines, which a server sends as a multiline batch,
    /// and events too long for one line.
    fn put(chat: &mut Chat, nick: &str, time: &str, id: &str, kind: crate::chat::EntryKind) {
        let at = chat.position("#api").unwrap();
        let mention = match &kind {
            crate::chat::EntryKind::Message(text) => crate::names::names_person(text, "me"),
            _ => false,
        };
        let entry = Entry {
            time: format!("2026-10-30T{time}Z").parse().unwrap(),
            id: Some(id.into()),
            from: nick.into(),
            owner: None,
            kind,
            history: false,
            mention,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
            sealed: None,
        };
        chat.conversations[at].entries.push(entry);
    }

    fn said_in_api(chat: &mut Chat, nick: &str, time: &str, id: &str, text: &str) {
        put(
            chat,
            nick,
            time,
            id,
            crate::chat::EntryKind::Message(text.into()),
        );
    }

    #[test]
    fn code_blocks_come_highlighted() {
        let mut chat = chat();
        chat.set_highlighter(Some(crate::chat::highlighted_lines));
        said_in_api(
            &mut chat,
            "alice",
            "09:00:00",
            "m1",
            "```rust\nfn main() {}\n```",
        );
        let lines = api(&chat);
        let [
            RichBlock::Code {
                language,
                lines: code,
                ..
            },
        ] = lines[0].rich.as_slice()
        else {
            panic!("a code block: {:?}", lines[0].rich);
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(code[0].tokens[0].text, "fn");
        assert_eq!(code[0].tokens[0].role, "syntax-keyword");
    }

    #[test]
    fn long_messages_are_cut_with_one_control() {
        let mut chat = chat();
        let long: Vec<String> = (1..=12).map(|n| format!("line {n}")).collect();
        said_in_api(&mut chat, "alice", "09:00:00", "m1", &long.join("\n"));
        let first = api(&chat).remove(0);
        let more = first.more.clone().expect("a control");
        assert!(!more.open);
        assert_eq!(more.hidden, 6);
        assert_eq!(more.label, "Show 6 more lines");
        assert_eq!(more.spoken, "Show 6 more lines of alice's message");
        assert_eq!(more.code_lines, None);
        // The whole text is kept for copying, whatever shows.
        assert_eq!(first.text, long.join("\n"));
        chat.toggle_more(&first.key, more.open);
        let opened = api(&chat).remove(0);
        let more = opened.more.expect("a control");
        assert!(more.open);
        assert_eq!(more.label, "Show less");
    }

    #[test]
    fn counts_reactions_and_marks_ours() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("m1"), ":alice!u@h PRIVMSG #api :ship it?"),
                "@+draft/react=👍;+draft/reply=m1 :bob!u@h TAGMSG #api",
                "@+draft/react=👍;+draft/reply=m1 :me!u@h TAGMSG #api",
                "@+draft/react=🎉;+draft/reply=m1 :bob!u@h TAGMSG #api",
            ],
        );
        assert_eq!(
            api(&chat)[0].reactions,
            [
                LineReaction {
                    emoji: "👍".into(),
                    count: 2,
                    mine: true,
                    label: "👍 by bob, me".into(),
                },
                LineReaction {
                    emoji: "🎉".into(),
                    count: 1,
                    mine: false,
                    label: "🎉 by bob".into(),
                },
            ]
        );
    }

    #[test]
    fn an_event_offers_what_a_message_does_but_deleting() {
        let mut chat = chat();
        // An event our own machine's zircd said, as every zirc event is: a PRIVMSG the
        // server names, which a reaction names in turn.
        let intent = named(
            "cc-1",
            "checkout",
            Event::Intent {
                paths: vec!["web/src".into()],
                summary: "tidy the form".into(),
                expires: None,
                lasts: None,
            },
        );
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("e1"), &zirc("me", "#api", &intent)),
                "@+draft/react=👀;+draft/reply=e1 :bob!u@h TAGMSG #api",
                &at("09:01:00", Some("m1"), ":me!u@h PRIVMSG #api :mine"),
                &at("09:02:00", Some("m2"), ":bob!u@h PRIVMSG #api :theirs"),
                &at("09:03:00", None, ":carol!u@h JOIN #api"),
            ],
        );
        let lines = api(&chat);
        let event = line(&lines, "tidy the form");
        assert_eq!(event.kind, LineKind::Event);
        assert!(event.mine);
        assert_eq!(
            event.actions,
            LineActions {
                react: true,
                reply: true,
                copy_link: true,
                edit: false,
                delete: false,
                details: true,
            }
        );
        assert_eq!(
            event.reactions,
            [LineReaction {
                emoji: "👀".into(),
                count: 1,
                mine: false,
                label: "👀 by bob".into(),
            }]
        );
        assert!(line(&lines, "mine").actions.delete);
        let theirs = line(&lines, "theirs").actions;
        assert!(theirs.react && theirs.reply && theirs.copy_link && !theirs.delete);
        assert_eq!(line(&lines, "joined").actions, LineActions::default());
    }

    #[test]
    fn quotes_what_a_reply_answers_while_it_is_kept() {
        let mut chat = chat();
        let profiles: zirc_proto::profile::Profiles =
            serde_json::from_str(r#"{"alice": {"name": "Alice Liddell", "avatar": "3"}}"#).unwrap();
        chat.take_profiles(&profiles, "https://zirc.dev/");
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("acme".into()));
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("m1"),
                    ":alice!u@h PRIVMSG #api :the login bug",
                ),
                &at("09:01:00", Some("m2"), ":bob!u@h PRIVMSG #api :on it")
                    .replace(";msgid=m2", ";msgid=m2;+draft/reply=m1"),
                &at("09:02:00", Some("m3"), ":bob!u@h PRIVMSG #api :and that")
                    .replace(";msgid=m3", ";msgid=m3;+draft/reply=gone"),
            ],
        );
        let lines = api(&chat);
        assert_eq!(lines[0].quote, None);
        assert_eq!(
            line(&lines, "on it").quote,
            Some(LineQuote {
                id: "m1".into(),
                from: "Alice Liddell".into(),
                text: "the login bug".into(),
                face: Some(Face::Person {
                    nick: "alice".into(),
                    initials: "AL".into(),
                    hue: crate::chat::nick_degrees("alice"),
                    photo: Some("https://zirc.dev/m/acme/@alice?v=3".into()),
                }),
            })
        );
        // One no longer kept: its id alone, for "a reply to an earlier message".
        assert_eq!(
            line(&lines, "and that").quote,
            Some(LineQuote {
                id: "gone".into(),
                from: String::new(),
                text: String::new(),
                face: None,
            })
        ); // As text, for an app that draws it so.
        assert_eq!(
            line(&lines, "on it").quote.as_ref().unwrap().line(),
            "↪ Alice Liddell: the login bug"
        );
        assert_eq!(
            line(&lines, "and that").quote.as_ref().unwrap().line(),
            format!("↪ {}", crate::chat::EARLIER_REPLY)
        );
        // The bar over the composer while a reply is written says the same.
        let conversation = chat.conversation("#api").unwrap();
        assert_eq!(chat.quoted(conversation, "m1"), line(&lines, "on it").quote);
        assert_eq!(chat.quoted(conversation, "gone"), None);
        // In every app's words: who, bold, and what they said; an earlier message once gone.
        let bar = ReplyBar::of(chat.quoted(conversation, "m1"));
        assert_eq!(bar.from.as_deref(), Some("Alice Liddell"));
        assert_eq!(bar.line(), "Replying to Alice Liddell: the login bug");
        let gone = ReplyBar::of(chat.quoted(conversation, "gone"));
        assert_eq!(
            (gone.line(), gone.from),
            ("Replying to an earlier message".to_owned(), None)
        );
        let gone = ReplyBar::of(line(&lines, "and that").quote.clone());
        assert_eq!(gone.said(), crate::chat::EARLIER_MESSAGE);
        // Messages are under who said them.
        assert!(lines.iter().all(|line| line.shape == RowShape::Message));
    }

    #[test]
    fn an_event_says_its_own_words_when_it_has_no_verb_or_details() {
        let mut chat = chat();
        chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
        let commit = named(
            "cc-1",
            "checkout",
            Event::Commit {
                branch: "main".into(),
                sha: "abc1234".into(),
                subject: "Fix login".into(),
                paths: Vec::new(),
                repo: None,
            },
        );
        feed(
            &mut chat,
            &[&at(
                "09:00:00",
                Some("e1"),
                &zirc("me-mbp", "#api", &commit),
            )],
        );
        let lines = api(&chat);
        let system = lines[0].system.clone().unwrap();
        assert_eq!(system.words(&lines[0].text), system.details);
        let bare = SystemLine {
            verb: String::new(),
            details: String::new(),
            ..system
        };
        assert_eq!(bare.words("lease taken"), "lease taken");
    }

    #[expect(clippy::too_many_lines, reason = "one story, told step by step")]
    #[test]
    fn an_event_comes_in_parts() {
        let mut chat = chat();
        chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
        let repo = "https://github.com/acme/api";
        let sha = "abc1234def5678abc1234def5678abc1234def56";
        let commit = named(
            "cc-1",
            "checkout",
            Event::Commit {
                branch: "main".into(),
                sha: sha.into(),
                subject: "Fix login".into(),
                paths: vec!["web/src/login.ts".into()],
                repo: Some(repo.into()),
            },
        );
        let push = named(
            "cc-1",
            "checkout",
            Event::Push {
                branch: "main".into(),
                head: sha.into(),
                forced: false,
                subjects: vec!["Fix login".into()],
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: Some(repo.into()),
                on: None,
            },
        );
        let to = named("cc-1", "checkout", text("cc-598aa1: please rebase"));
        let intent = named(
            "cc-1",
            "checkout",
            Event::Intent {
                paths: vec!["web/src".into()],
                summary: "tidy the form".into(),
                expires: None,
                lasts: None,
            },
        );
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("e1"), &zirc("me-mbp", "#api", &commit)),
                &at("09:01:00", Some("e2"), &zirc("me-mbp", "#api", &push)),
                &at("09:02:00", Some("e3"), &zirc("me-mbp", "#api", &to)),
                &at("09:03:00", Some("e4"), &zirc("me-mbp", "#api", &intent)),
            ],
        );
        let lines = api(&chat);
        assert_eq!(lines.len(), 3, "the push folds into its commit: {lines:#?}");
        let made = &lines[0];
        assert_eq!(made.kind, LineKind::Event);
        let system = made.system.as_ref().unwrap();
        assert_eq!(system.icon, Icon::Commit);
        assert_eq!(system.glyph, "●");
        assert_eq!(system.verb, "committed and pushed to main");
        assert_eq!(system.lead, "● committed and pushed to main");
        assert_eq!(system.actor, "checkout");
        assert_eq!(
            system.session,
            Some(SessionName {
                id: "cc-1".into(),
                label: "checkout".into()
            })
        );
        // A commit's paths aren't said in its line.
        assert!(system.paths.is_empty());
        assert_eq!(made.clock, "09:00");
        assert_eq!(system.until_clock.as_deref(), Some("09:01"));
        assert!(!system.card);
        assert_eq!(
            (made.shape, system.body),
            (RowShape::Brief, SystemBody::Parts)
        );
        // The branch links to its page on the forge.
        assert!(system.verb_links.iter().any(|span| span.text == "main"
            && span.kind
                == RichSpanKind::Link {
                    url: crate::chat::branch_link(repo, "main")
                }));
        // The details link the commit, when they name it; else nothing links.
        assert!(
            system.details_links.is_empty()
                || system
                    .details_links
                    .iter()
                    .any(|span| matches!(span.kind, RichSpanKind::Link { .. }))
        );
        let speaker = made.speaker.as_ref().unwrap();
        assert_eq!(speaker.session.as_deref(), Some("cc-1"));
        assert_eq!(speaker.detail.as_deref(), Some("your agent"));
        // A post to another session says whom.
        let post = line(&lines, "please rebase");
        let system = post.system.as_ref().unwrap();
        assert_eq!(system.icon, Icon::Post);
        assert_eq!(system.lead, "");
        assert_eq!(
            system.to,
            Some(SessionName {
                id: "cc-598aa1".into(),
                label: "cc-598aa1".into()
            })
        );
        assert_eq!(system.details, "please rebase");
        assert!(system.details_links.is_empty());
        assert_eq!(system.until_clock, None);
        // Its words are blocks, as a message's are, though plain, after whom it's to, which
        // links to that session.
        assert_eq!(
            (post.shape, system.body),
            (RowShape::System, SystemBody::Blocks)
        );
        assert_eq!(rich_text(post), "→ cc-598aa1 please rebase");
        let RichBlock::Paragraph { spans } = &post.rich[0] else {
            panic!("{:?}", post.rich)
        };
        assert!(matches!(&spans[1].kind, RichSpanKind::Session { id, .. } if id == "cc-598aa1"));
        assert!(post.more.is_none());
        let about = lines[2].system.as_ref().unwrap();
        assert_eq!(about.icon, Icon::Intent);
        assert_eq!(about.verb, "is about to change");
        assert_eq!(about.details, "tidy the form");
        assert_eq!(about.paths, ["web/src"]);
        // Its paths are listed under what it's for, which starts a line of its own.
        assert!(about.paths_listed && about.details_apart);
        assert_eq!(
            (about.marker_tone.as_str(), about.details_tone.as_str()),
            ("muted", "body")
        );
        // A commit's line has no paths, and its details follow its lead.
        let made = lines[0].system.as_ref().unwrap();
        assert!(!made.paths_listed && !made.details_apart);
    }

    #[test]
    fn the_core_says_which_lines_are_brief_and_heads_the_run_after_one() {
        let mut reacted = chat();
        let intent = |summary: &str| {
            named(
                "cc-1",
                "checkout",
                Event::Intent {
                    paths: vec!["web/src".into()],
                    summary: summary.into(),
                    expires: None,
                    lasts: None,
                },
            )
        };
        let said = named("cc-1", "checkout", text("all done"));
        feed(
            &mut reacted,
            &[
                &at(
                    "09:00:00",
                    Some("e1"),
                    &zirc("me-mbp", "#api", &intent("one")),
                ),
                &at(
                    "09:01:00",
                    Some("e2"),
                    &zirc("me-mbp", "#api", &intent("two")),
                ),
                &at("09:02:00", Some("e3"), &zirc("me-mbp", "#api", &said)),
                "@time=2026-10-30T09:03:00.000Z;+draft/react=👀;+draft/reply=e2 :bob!u@h TAGMSG #api",
            ],
        );
        let lines = api(&reacted);
        let brief = |at: usize| lines[at].system.as_ref().unwrap().brief.is_some();
        // Routine work is brief until someone reacts to it (the user's choice, 14 Nov 2026).
        assert!(brief(0), "{lines:#?}");
        assert!(!brief(1), "a reacted-to line is whole: {lines:#?}");
        // What follows a brief line heads itself; what follows a whole one carries on.
        assert!(!lines[1].continues && lines[2].continues, "{lines:#?}");
        let mut unreacted = chat();
        feed(
            &mut unreacted,
            &[
                &at(
                    "09:00:00",
                    Some("e1"),
                    &zirc("me-mbp", "#api", &intent("one")),
                ),
                &at("09:02:00", Some("e3"), &zirc("me-mbp", "#api", &said)),
            ],
        );
        let lines = api(&unreacted);
        assert!(!lines[1].continues, "{lines:#?}");
        // In the agent's own conversation, its work is whole.
        let own = unreacted.agent_lines("me-mbp/cc-1", utc());
        assert!(
            own.iter()
                .filter_map(|line| line.system.as_ref())
                .all(|system| system.brief.is_none()),
            "{own:#?}"
        );
    }

    #[test]
    fn a_push_of_several_commits_lists_them_apart() {
        let mut chat = chat();
        chat.set_threshold("#api", crate::chat::COMMITS_AND_PUSHES_SHOW_AT);
        let push = named(
            "cc-1",
            "checkout",
            Event::Push {
                branch: "main".into(),
                head: "abc1234def".into(),
                forced: false,
                subjects: vec!["One".into(), "Two".into()],
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: None,
                on: None,
            },
        );
        feed(
            &mut chat,
            &[&at("09:00:00", Some("p1"), &zirc("me-mbp", "#api", &push))],
        );
        let lines = api(&chat);
        let pushed = lines[0].system.as_ref().unwrap();
        assert_eq!(pushed.verb, "pushed main to abc1234 (2 commits)");
        assert_eq!(pushed.details, "One\nTwo");
        // Lines of their own, with no paths under them: still apart from the lead.
        assert!(pushed.details_apart && !pushed.paths_listed);
    }

    #[test]
    fn a_posts_words_are_drawn_and_cut_as_a_messages() {
        let mut chat = chat();
        let steps: Vec<String> = (1..=12).map(|n| format!("- step {n}")).collect();
        let long = named("cc-1", "checkout", text(&steps.join("\n")));
        let bold = named("cc-1", "checkout", text("cc-598aa1: **please** rebase"));
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("e1"), &zirc("me-mbp", "#api", &long)),
                &at("09:01:00", Some("e2"), &zirc("me-mbp", "#api", &bold)),
            ],
        );
        let lines = api(&chat);
        let cut = &lines[0];
        assert_eq!(cut.kind, LineKind::Event);
        assert!(matches!(cut.rich.as_slice(), [RichBlock::Items { .. }]));
        let more = cut.more.as_ref().expect("a control");
        assert_eq!(more.label, "Show 6 more lines");
        assert_eq!(more.spoken, "Show 6 more lines of your agent's message");
        // Addressed to a session, its words follow `→ ` and the session.
        let to = &lines[1];
        assert_eq!(
            to.system.as_ref().and_then(|system| system.to.as_ref()),
            Some(&SessionName {
                id: "cc-598aa1".into(),
                label: "cc-598aa1".into()
            })
        );
        let [RichBlock::Paragraph { spans }] = to.rich.as_slice() else {
            panic!("one paragraph: {:?}", to.rich);
        };
        assert_eq!(spans[0].text, "→ ");
        assert!(
            matches!(&spans[1].kind, RichSpanKind::Session { id, label, .. } if id == "cc-598aa1" && label == "cc-598aa1")
        );
        assert!(spans[3].bold && spans[3].text == "please");
    }

    fn ci_landing(text: &str) -> Message {
        Message::new(
            "ci",
            Event::Custom {
                kind: zirc_proto::ci::LANDING_KIND.into(),
                data: serde_json::json!({
                    "session": "cc-1",
                    "landing": "monorepo/771",
                    "build": "771",
                    "url": "https://buildkite.com/zirc/monorepo/builds/771",
                    "branch": "fix-login",
                }),
                text: text.into(),
            },
        )
    }

    #[test]
    fn a_reply_to_a_ci_line_quotes_what_the_line_says() {
        let mut chat = chat();
        let failed = ci_landing("cc-1: build 771 failed: published code");
        put(
            &mut chat,
            "zirc-ci",
            "09:00:00",
            "l1",
            crate::chat::EntryKind::Zirc(failed),
        );
        feed(
            &mut chat,
            &[&at(
                "09:01:00",
                Some("m2"),
                ":bob!u@h PRIVMSG #api :two flaky tests",
            )
            .replace(";msgid=m2", ";msgid=m2;+draft/reply=l1")],
        );
        let lines = api(&chat);
        let quote = line(&lines, "two flaky tests").quote.clone().unwrap();
        assert_eq!(
            (quote.id.as_str(), quote.text.as_str()),
            ("l1", "cc-1: build 771 failed: published code")
        );
    }

    #[test]
    fn a_landing_is_one_line_with_its_outcome() {
        let mut chat = chat();
        let landed = ci_landing("cc-1: landed fix-login as abc1234 (build 771)");
        put(
            &mut chat,
            "zirc-ci",
            "09:00:00",
            "l1",
            crate::chat::EntryKind::Zirc(landed),
        );
        let lines = api(&chat);
        let landed = &lines[0];
        assert_eq!(landed.kind, LineKind::Landing);
        assert!(!landed.mine);
        assert_eq!(
            landed.link.as_deref(),
            Some("https://buildkite.com/zirc/monorepo/builds/771")
        );
        assert!(landed.text.contains("fix-login") && landed.text.contains("build 771"));
        let system = landed.system.as_ref().unwrap();
        assert_eq!(system.outcome, Some(Icon::Landed));
        assert!(system.card);
        assert_eq!(system.buttons, None);
        assert_eq!(system.card_version, None);
        assert_eq!(system.session.as_ref().map(|s| s.id.as_str()), Some("cc-1"));
        // Its build links to its page.
        let [RichBlock::Paragraph { spans }, ..] = landed.rich.as_slice() else {
            panic!("the card's line: {:?}", landed.rich);
        };
        assert!(
            spans
                .iter()
                .any(|span| span.text == "build 771"
                    && matches!(span.kind, RichSpanKind::Link { .. }))
        );
        assert!(landed.speaker.as_ref().is_some_and(|speaker| speaker.app));
    }

    fn card(actions: Vec<Action>) -> Card {
        Card {
            state: State::Pending,
            actions,
            ..Card::new("deploy/42", "web · deploy 42")
        }
    }

    fn action(id: &str, who: Who) -> Action {
        Action {
            id: id.into(),
            label: id.to_uppercase(),
            who,
            confirm: Some(format!("{id}?")),
        }
    }

    #[test]
    fn a_card_shows_us_the_buttons_we_may_press() {
        let mut chat = chat();
        let card = card(vec![
            action("approve", Who::Members),
            action("cancel", Who::Owners),
        ]);
        let posted = Message::new(
            "deploy",
            Event::Custom {
                kind: "x-zirc/deploy".into(),
                data: card.as_data(),
                text: "web · deploy 42 pending".into(),
            },
        );
        put(
            &mut chat,
            "zirc-deploy",
            "09:00:00",
            "c1",
            crate::chat::EntryKind::Zirc(posted),
        );
        let lines = api(&chat);
        let system = lines[0].system.as_ref().unwrap();
        assert!(system.card);
        let version = system.card_version.clone().expect("a card's version");
        assert_eq!(
            system.buttons,
            Some(CardButtons {
                sender: "zirc-deploy".into(),
                card: "deploy/42".into(),
                key: version,
                actions: vec![CardAction {
                    id: "approve".into(),
                    label: "APPROVE".into(),
                    confirm: Some("approve?".into()),
                    primary: true,
                }],
            })
        );
        assert_eq!(system.outcome, None);
    }

    /// What `line`'s rich text says, span by span.
    fn rich_text(line: &Line) -> String {
        let mut text = String::new();
        RichBlock::each_span(&line.rich, &mut |span| text.push_str(&span.text));
        text
    }

    #[test]
    fn a_cards_outcome_is_its_state_whatever_its_words_say() {
        // An integration's card that failed and then passed: its words said "failed" once,
        // which read as CI's words alone would be a failed deploy.
        let mut chat = chat();
        let post = |chat: &mut Chat, time: &str, id: &str, card: &Card| {
            let state = if card.state == State::Failed {
                "failed"
            } else {
                "passed"
            };
            let posted = Message::new(
                "deploy",
                Event::Custom {
                    kind: "x-acme/deploy".into(),
                    data: card.as_data(),
                    text: format!("{} {state}", card.title),
                },
            );
            put(
                chat,
                "zirc-alerts",
                time,
                id,
                crate::chat::EntryKind::Zirc(posted),
            );
        };
        let mut now = card(Vec::new());
        now.state = State::Failed;
        post(&mut chat, "09:00:00", "c1", &now);
        assert_eq!(
            api(&chat)[0].system.as_ref().unwrap().outcome,
            Some(Icon::Failed)
        );
        now.state = State::Passed;
        post(&mut chat, "09:02:00", "c2", &now);
        assert_eq!(
            api(&chat)[0].system.as_ref().unwrap().outcome,
            Some(Icon::Landed)
        );
    }

    #[test]
    fn ci_saying_a_deploy_failed_fails_its_landing() {
        let mut chat = chat();
        let said = |chat: &mut Chat, time: &str, id: &str, kind: &str, data, text: &str| {
            let posted = Message::new(
                "ci",
                Event::Custom {
                    kind: kind.into(),
                    data,
                    text: text.into(),
                },
            );
            put(
                chat,
                "zirc-ci",
                time,
                id,
                crate::chat::EntryKind::Zirc(posted),
            );
        };
        let landing = serde_json::json!({"landing": "monorepo/771", "build": "771"});
        let outcome = |chat: &Chat| api(chat)[0].system.as_ref().unwrap().outcome;
        said(
            &mut chat,
            "09:00:00",
            "l1",
            crate::chat::LANDING_KIND,
            landing.clone(),
            "cc-1: landed fix-login as abc1234 (build 771)",
        );
        assert_eq!(outcome(&chat), Some(Icon::Landed));
        // CI's words alone: the deploy failed, so the landing's card did.
        said(
            &mut chat,
            "09:05:00",
            "l2",
            zirc_proto::ci::DEPLOY_KIND,
            landing,
            "cc-1: deploy failed: web in build 771",
        );
        assert_eq!(outcome(&chat), Some(Icon::Failed));
    }

    #[test]
    fn a_cards_updates_show_under_it_and_its_latest_is_said_where_it_came() {
        let mut chat = chat();
        let post = |chat: &mut Chat, time: &str, id: &str, card: &Card| {
            let posted = Message::new(
                "deploy",
                Event::Custom {
                    kind: "x-zirc/deploy".into(),
                    data: card.as_data(),
                    text: card.title.clone(),
                },
            );
            put(
                chat,
                "zirc-deploy",
                time,
                id,
                crate::chat::EntryKind::Zirc(posted),
            );
        };
        let mut now = card(Vec::new());
        post(&mut chat, "09:00:00", "c1", &now);
        now.state = State::Running;
        post(&mut chat, "09:02:00", "c2", &now);
        put(
            &mut chat,
            "bob",
            "09:03:00",
            "m1",
            crate::chat::EntryKind::Message("looks slow".into()),
        );
        now.state = State::Failed;
        now.note = Some("error: timed out".into());
        post(&mut chat, "09:04:00", "c3", &now);
        // Only a field changing is no update of its own.
        now.fields.push(zirc_proto::card::Field {
            label: "commit".into(),
            value: "abc1234".into(),
        });
        post(&mut chat, "09:05:00", "c4", &now);
        now.note = Some("retried by mattias".into());
        post(&mut chat, "09:06:00", "c5", &now);
        now.state = State::Passed;
        now.note = None;
        post(&mut chat, "09:08:00", "c6", &now);
        let lines = api(&chat);
        let shown: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            shown,
            vec![
                "web · deploy 42 passed",
                "looks slow",
                "zirc-deploy updated web · deploy 42: failed → passed ↑",
            ],
        );
        // The card: its last three updates, the first folded away.
        let card = &lines[0];
        assert!(
            rich_text(card).ends_with(
                "09:04 running → failed · error: timed out\
                 09:06 failed · retried by mattias\
                 09:08 failed → passed"
            ),
            "{}",
            rich_text(card)
        );
        assert!(!rich_text(card).contains("09:02"));
        // Its trail last, a stub an update, which every app draws with a tear before it.
        let Some(RichBlock::Trail { stubs }) = card.rich.last() else {
            panic!("the trail comes last: {:?}", card.rich.last());
        };
        assert_eq!(stubs.len(), 3);
        assert!(stubs[0].spans[0].bold && stubs[0].spans[0].text == "09:04");
        let more = card.more.as_ref().expect("its earlier updates fold away");
        assert_eq!(
            (more.label.as_str(), more.hidden, more.open),
            ("Show 1 earlier update", 1, false)
        );
        assert_eq!(more.spoken, "Show 1 earlier update of Deploy's card");
        chat.toggle_more(&card.key, more.open);
        let card = &api(&chat)[0];
        assert!(rich_text(card).contains("09:02 pending → running09:04"));
        assert_eq!(card.more.as_ref().unwrap().label, "Show fewer updates");
        // The pointer: a quiet line that jumps to the card.
        let pointer = lines[2].system.as_ref().unwrap();
        assert_eq!(pointer.points_to.as_deref(), Some("c1"));
        assert_eq!(lines[2].shape, RowShape::Pointer);
        assert_eq!(lines[0].shape, RowShape::System);
        assert_eq!(pointer.icon, Icon::Landed);
        assert_eq!(lines[0].system.as_ref().unwrap().points_to, None);
    }

    #[test]
    fn cards_updated_one_after_another_point_in_the_order_they_came() {
        // As one deploy updating six landings' cards: their pointers share one place, and
        // must keep their order, not a map's, which changed on every redraw.
        let mut chat = chat();
        let post = |chat: &mut Chat, time: &str, id: String, card: &Card| {
            let posted = Message::new(
                "deploy",
                Event::Custom {
                    kind: "x-zirc/deploy".into(),
                    data: card.as_data(),
                    text: card.title.clone(),
                },
            );
            put(
                chat,
                "zirc-ci",
                time,
                &id,
                crate::chat::EntryKind::Zirc(posted),
            );
        };
        let cards: Vec<Card> = (1..=6)
            .map(|n| Card {
                id: format!("deploy/{n}"),
                title: format!("landing {n}"),
                ..card(Vec::new())
            })
            .collect();
        for (n, card) in cards.iter().enumerate() {
            post(&mut chat, &format!("09:0{n}:00"), format!("c{n}"), card);
        }
        put(
            &mut chat,
            "bob",
            "09:10:00",
            "m1",
            crate::chat::EntryKind::Message("deploying".into()),
        );
        for (n, card) in cards.iter().enumerate() {
            let passed = Card {
                state: State::Passed,
                ..card.clone()
            };
            post(&mut chat, &format!("09:2{n}:00"), format!("u{n}"), &passed);
        }
        let shown: Vec<String> = api(&chat)
            .iter()
            .skip(7)
            .map(|line| line.text.clone())
            .collect();
        let expected: Vec<String> = (1..=6)
            .map(|n| format!("zirc-ci updated landing {n}: pending → passed ↑"))
            .collect();
        assert_eq!(shown, expected);
    }

    #[test]
    fn a_card_updated_with_nothing_after_it_needs_no_pointer() {
        let mut chat = chat();
        let mut now = card(Vec::new());
        for (time, id, state) in [
            ("09:00:00", "c1", State::Pending),
            ("09:01:00", "c2", State::Running),
        ] {
            now.state = state;
            let posted = Message::new(
                "deploy",
                Event::Custom {
                    kind: "x-zirc/deploy".into(),
                    data: now.as_data(),
                    text: now.title.clone(),
                },
            );
            put(
                &mut chat,
                "zirc-deploy",
                time,
                id,
                crate::chat::EntryKind::Zirc(posted),
            );
        }
        let lines = api(&chat);
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert!(rich_text(&lines[0]).ends_with("09:01 pending → running"));
        assert_eq!(lines[0].more, None, "one update shows whole");
    }

    #[test]
    fn people_coming_and_going_are_one_quiet_line() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at("09:00:00", None, ":carol!u@h JOIN #api"),
                &at("09:00:00", None, ":dave!u@h JOIN #api"),
                &at("09:05:00", None, ":alice!u@h PART #api :bye"),
            ],
        );
        let lines = api(&chat);
        assert_eq!(lines.len(), 1, "{lines:#?}");
        let came = &lines[0];
        assert_eq!(came.kind, LineKind::Membership);
        assert_eq!(came.text, "carol and dave joined, alice left");
        assert_eq!(came.from, "");
        assert!(came.speaker.is_none() && came.presence.is_none());
        let system = came.system.as_ref().unwrap();
        // One quiet line, in every app (the user's choice, 16 Nov 2026).
        assert_eq!(came.shape, RowShape::Quiet);
        assert_eq!(system.icon, Icon::Join);
        assert_eq!(system.verb, came.text);
        let nine = Utc
            .with_ymd_and_hms(2026, 10, 30, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(came.key, format!("{nine}:0"));
    }

    fn shared(mime: &str, url: &str, size: (Option<u32>, Option<u32>)) -> Message {
        Message::new(
            "desktop",
            Event::File(zirc_proto::File {
                url: url.into(),
                name: "IMG_2041.jpeg".into(),
                mime: mime.into(),
                size: 2_345_678,
                width: size.0,
                height: size.1,
                duration_ms: None,
            }),
        )
    }

    #[test]
    fn a_hosted_file_is_drawn_and_another_only_said() {
        let mut chat = chat();
        let hosted = "https://zirc.dev/m/acme/7Hq2xV9k";
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("f1"),
                    &zirc(
                        "alice",
                        "#api",
                        &shared("image/jpeg", hosted, (Some(4032), Some(3024))),
                    ),
                ),
                &at(
                    "09:01:00",
                    Some("f2"),
                    &zirc("alice", "#api", &shared("video/mp4", hosted, (None, None))),
                ),
                &at(
                    "09:02:00",
                    Some("f3"),
                    &zirc("alice", "#api", &shared("text/plain", hosted, (None, None))),
                ),
                &at(
                    "09:03:00",
                    Some("f4"),
                    &zirc(
                        "alice",
                        "#api",
                        &shared("image/png", "https://evil.example/x", (Some(10), Some(10))),
                    ),
                ),
            ],
        );
        // With no web app said, nothing is loaded.
        assert!(api(&chat).iter().all(|line| line.kind == LineKind::Note));
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("acme".into()));
        let lines = api(&chat);
        let image = &lines[0];
        assert_eq!(image.kind, LineKind::File);
        assert!(image.speaker.is_some());
        let file = image.file.as_ref().unwrap();
        assert_eq!(file.shape, FileShape::Image);
        assert_eq!(file.url.as_deref(), Some(hosted));
        assert_eq!((file.width, file.height), (Some(320), Some(240)));
        assert_eq!(file.size, 2_345_678);
        assert_eq!(file.size_text, "2.3 MB");
        assert_eq!(file.mime, "image/jpeg");
        let srcset = file.srcset.as_deref().unwrap();
        assert!(
            srcset.starts_with(&format!("{hosted}?w=64 64w, ")),
            "{srcset}"
        );
        assert_eq!(file.poster, None);
        let video = lines[1].file.as_ref().unwrap();
        assert_eq!(video.shape, FileShape::Video);
        assert_eq!(video.poster, Some(format!("{hosted}?w=1280")));
        assert_eq!((video.width, video.height), (None, None));
        let text = lines[2].file.as_ref().unwrap();
        assert_eq!(text.shape, FileShape::Text);
        assert_eq!((text.srcset.as_ref(), text.poster.as_ref()), (None, None));
        let elsewhere = &lines[3];
        assert_eq!(elsewhere.kind, LineKind::Note);
        assert_eq!(elsewhere.file, None);
        assert_eq!(
            elsewhere.text,
            "alice shared IMG_2041.jpeg (2.3 MB) from elsewhere, not shown"
        );
    }

    #[test]
    fn a_self_hosted_organisations_files_are_drawn_from_its_own_server() {
        let mut chat = chat();
        let own = "https://irc.acme.com/m/7Hq2xV9k";
        let web = "https://zirc.dev/m/acme/7Hq2xV9k";
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("f1"),
                    &zirc("alice", "#api", &shared("video/mp4", own, (None, None))),
                ),
                &at(
                    "09:01:00",
                    Some("f2"),
                    &zirc(
                        "alice",
                        "#api",
                        &shared("image/png", web, (Some(10), Some(10))),
                    ),
                ),
            ],
        );
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("acme".into()));
        chat.set_files_site(Some("https://irc.acme.com".into()));
        let lines = api(&chat);
        assert_eq!(lines[0].kind, LineKind::File);
        assert_eq!(lines[0].file.as_ref().unwrap().url.as_deref(), Some(own));
        // Its files are its server's: one at the web app isn't its own.
        assert_eq!(lines[1].kind, LineKind::Note);
        // People's pictures still come from the web app.
        assert_eq!(chat.picture_web(), Some("https://zirc.dev"));
    }

    #[test]
    fn people_only_keeps_what_people_said() {
        let mut chat = chat();
        let post = named("cc-1", "checkout", text("done: the form"));
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("m1"), ":alice!u@h PRIVMSG #api :hi"),
                &at("09:01:00", Some("e1"), &zirc("me-mbp", "#api", &post)),
            ],
        );
        assert_eq!(chat.lines("#api", utc(), false).len(), 2);
        let people: Vec<String> = chat
            .lines("#api", utc(), true)
            .into_iter()
            .map(|line| line.text)
            .collect();
        assert_eq!(people, ["hi"]);
        assert!(chat.lines("#nowhere", utc(), false).is_empty());
    }

    #[test]
    fn an_agents_lines_carry_their_rooms_and_break_runs_between_them() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[":me-mbp!u@machine/me JOIN #api", ":me!u@h JOIN #api/agents"],
        );
        let checkout = |body: &str| named("cc-1", "checkout", text(body));
        feed(
            &mut chat,
            &[
                &at(
                    "10:00:00",
                    Some("a1"),
                    &zirc("me-mbp", "#api/agents", &checkout("taking: the login page")),
                ),
                &at(
                    "10:00:30",
                    Some("a2"),
                    &zirc("me-mbp", "#api", &checkout("first in api")),
                ),
                &at(
                    "10:01:00",
                    Some("a3"),
                    &zirc("me-mbp", "#api", &checkout("second in api")),
                ),
                &at(
                    "10:02:00",
                    Some("a4"),
                    &zirc("me-mbp", "#api/agents", &checkout("back in agents")),
                ),
            ],
        );
        let lines = chat.agent_lines("me-mbp/cc-1", utc());
        let rooms: Vec<(Option<&str>, bool)> = lines
            .iter()
            .map(|line| (line.room.as_deref(), line.continues))
            .collect();
        assert_eq!(
            rooms,
            [
                (Some("#api/agents"), false),
                (Some("#api"), false),
                (Some("#api"), true),
                (Some("#api/agents"), false),
            ]
        );
        assert_eq!(lines[0].day.as_deref(), Some("Today"));
        // Its own lines leave whose agent it is to the header.
        assert!(lines.iter().all(|line| {
            line.speaker
                .as_ref()
                .is_some_and(|speaker| speaker.detail.is_none() && speaker.name == "checkout")
        }));
        // In a channel the same lines say whose agent it is.
        assert!(api(&chat).iter().any(|line| {
            line.speaker
                .as_ref()
                .is_some_and(|speaker| speaker.detail.is_some())
        }));
        assert!(chat.agent_lines("#api", utc()).is_empty());
    }

    #[test]
    fn offers_editing_our_own_words_for_twenty_minutes_and_marks_them_edited() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at("11:30:00", Some("m1"), ":me!u@h PRIVMSG #api :too old"),
                &at("11:45:00", Some("m2"), ":me!u@h PRIVMSG #api :teh fix"),
                &at("11:46:00", Some("m3"), ":bob!u@h PRIVMSG #api :theirs"),
                &at("11:47:00", Some("m4"), ":me!u@h NOTICE #api :a notice"),
                &at(
                    "11:50:00",
                    Some("m5"),
                    "@+draft/edit=m2 :me!u@h PRIVMSG #api :the fix",
                )
                .replace(" @", ";"),
            ],
        );
        let lines = api(&chat);
        assert!(!line(&lines, "too old").actions.edit);
        assert!(line(&lines, "too old").actions.delete);
        let fixed = line(&lines, "the fix");
        assert!(fixed.actions.edit && fixed.edited);
        assert!(!line(&lines, "theirs").actions.edit);
        assert!(!line(&lines, "a notice").actions.edit);
        assert_eq!(lines.iter().filter(|l| l.text.contains("fix")).count(), 1);
    }

    #[test]
    fn a_card_names_its_session_once() {
        // The branch called after its session: the session's name isn't said again.
        assert!(super::names_word(
            "checkout · build 418 · landed",
            "checkout"
        ));
        assert!(super::names_word("build 1418 failed on main: e2e", "main"));
        // A name only inside another word is a different one.
        assert!(!super::names_word(
            "checkout-v2 · build 418 · landed",
            "checkout"
        ));
        assert!(!super::names_word("build 1421 failed: e2e", "flaky-e2e"));
    }
}
