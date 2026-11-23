//! Drawing the [`App`]: a header with the conversation's topic, the conversations down the
//! left with their unread counts, the scrollback in the middle, the channel's members on the
//! right, and a status line and the input line at the bottom. Everything shown comes from
//! the app's state and [`rows::layout`]; nothing here changes either. [`target_at`] says
//! what's drawn where, for the mouse, from the same layout. Colours are the roles of
//! zirc's theme ([`zirc_theme::Theme`]), dark or light as the app's says.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr as _;

use crate::app::{App, SessionsPanel, Side, Target, Trimming};
use crate::find::Searching;
use crate::hyperlink;
use crate::jump::{self, Jumping};
use crate::picker::{self, Picker};
use crate::rows::{self, Row, Tone, fit, wrap};
use zirc_core::chat::{
    AGENT_TAG, APP_TAG, APPS_SECTION, Avatar, HeaderLineKind, Icon, Kind, OrgHeader, Presence,
    Profile, SearchHit, SearchScreen, away_label, members_label, mentions_label,
};
use zirc_core::emoji;
use zirc_theme::{Design, Rgb, Theme, Type};

/// The conversation list is hidden below this terminal width.
const SIDEBAR_FROM: u16 = 50;
/// The member list is shown from this terminal width.
const MEMBERS_FROM: u16 = 90;
/// The narrowest and widest the member list gets, with its border.
const MEMBERS_WIDTH: (u16, u16) = (18, 36);

/// A role's colour, for the terminal.
fn colour(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// Text in `colour`.
fn fg_colour(colour: Color) -> Style {
    Style::new().fg(colour)
}

/// An agent's colour, from its `hue` in degrees: as zirc's desktop app colours one, light
/// enough on a dark terminal and dark enough on a light one to read.
fn hue_colour(app: &App, hue: u16) -> Color {
    let ground = app.theme.ground;
    let dark = u16::from(ground.r) + u16::from(ground.g) + u16::from(ground.b) < 384;
    let (saturation, lightness) = (0.55_f32, if dark { 0.7_f32 } else { 0.36 });
    // HSL to RGB.
    let chroma = (1. - (2. * lightness - 1.).abs()) * saturation;
    let sector = f32::from(hue % 360) / 60.;
    let second = chroma * (1. - (sector % 2. - 1.).abs());
    let (red, green, blue) = match sector {
        sector if sector < 1. => (chroma, second, 0.),
        sector if sector < 2. => (second, chroma, 0.),
        sector if sector < 3. => (0., chroma, second),
        sector if sector < 4. => (0., second, chroma),
        sector if sector < 5. => (second, 0., chroma),
        _ => (chroma, 0., second),
    };
    let base = lightness - chroma / 2.;
    let byte = |part: f32| Rgb::byte(f64::from(part + base));
    Color::Rgb(byte(red), byte(green), byte(blue))
}

/// Text in a role's colour.
fn fg(rgb: Rgb) -> Style {
    Style::new().fg(colour(rgb))
}

/// The areas of the screen.
struct Areas {
    header: Rect,
    sidebar: Option<Rect>,
    scrollback: Rect,
    members: Option<Rect>,
    status: Rect,
    /// Who's typing, on the line under the input, while anyone is.
    typing: Option<Rect>,
    input: Rect,
}

fn areas(app: &App, area: Rect) -> Areas {
    let typing = u16::from(app.typing().is_some());
    let [header, middle, status, input, typing_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(input_height(app, area)),
        Constraint::Length(typing),
    ])
    .areas(area);
    let sidebar_width = if area.width >= SIDEBAR_FROM {
        sidebar_width(app)
    } else {
        0
    };
    let members_width = if area.width >= MEMBERS_FROM && app.current().kind == Kind::Channel {
        members_width(app, area.width)
    } else {
        0
    };
    let [sidebar, scrollback, members] = Layout::horizontal([
        Constraint::Length(sidebar_width),
        Constraint::Min(1),
        Constraint::Length(members_width),
    ])
    .areas(middle);
    Areas {
        header,
        sidebar: (sidebar_width > 0).then_some(sidebar),
        // A column of space either side of the text.
        scrollback: Rect {
            x: scrollback.x + 1,
            width: scrollback.width.saturating_sub(2),
            ..scrollback
        },
        members: (members_width > 0).then_some(members),
        status,
        typing: (typing > 0).then_some(typing_area),
        input,
    }
}

/// The size of the scrollback when the whole screen is `area`, for scrolling by pages.
#[must_use]
pub fn scrollback_size(app: &App, area: Rect) -> (u16, u16) {
    let scrollback = areas(app, area).scrollback;
    (scrollback.width, scrollback.height)
}

/// What's at `column` and `row` when the whole screen is `area`: a conversation in the list,
/// someone in the member list, or the scrollback. `None` for anywhere else.
#[must_use]
pub fn target_at(app: &App, area: Rect, column: u16, row: u16) -> Option<Target> {
    let at = Position { x: column, y: row };
    let areas = areas(app, area);
    if let Some(sidebar) = areas.sidebar.map(|area| sidebar_block().inner(area))
        && sidebar.contains(at)
    {
        return match app
            .sidebar()
            .into_iter()
            .nth(usize::from(row - sidebar.y))?
        {
            Side::Row { target, .. } => target,
            Side::Heading { shelf, .. } => Some(Target::Shelf(shelf)),
            // Each part of the switch takes its share of the width, the last what's left.
            Side::Tabs(tabs) => {
                let share = usize::from(sidebar.width) / tabs.len().max(1);
                let part = (usize::from(column - sidebar.x) / share.max(1)).min(tabs.len() - 1);
                tabs.get(part).map(|tab| Target::Tab(tab.section))
            }
            Side::Org(_) | Side::Empty { .. } => None,
        };
    }
    if let Some(members) = areas.members.map(|area| members_block().inner(area))
        && members.contains(at)
    {
        // The first row says how many there are.
        let index = usize::from(row - members.y).checked_sub(1)?;
        let people = app.chat.people_rows(&app.current().name);
        return Some(Target::Member(people.get(index)?.person.nick.clone()));
    }
    areas
        .scrollback
        .contains(at)
        .then(|| scrollback_target(app, areas.scrollback, column, row))
}

/// Draws the app on the whole frame.
///
/// What a draw asks of the core is timed, as `tui()`, by zirc-chat's speed budget
/// (`chat/tests/perf.rs`): a call added here or in [`App::sidebar`] is added there too.
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let theme = app.theme;
    // On the terminal's own background, whatever it is: the theme matches its colours to
    // whether that's dark or light.
    frame.render_widget(
        Block::new().style(fg(theme.body).bg(Color::Reset)),
        frame.area(),
    );
    let areas = areas(app, frame.area());
    header(frame, app, areas.header);
    if let Some(area) = areas.sidebar {
        sidebar(frame, app, area);
    }
    scrollback(frame, app, areas.scrollback);
    if let Some(area) = areas.members {
        members(frame, app, area);
    }
    status(frame, app, areas.status);
    if let (Some(area), Some(typing)) = (areas.typing, app.typing()) {
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {typing}"), fg(theme.muted))),
            area,
        );
    }
    input(frame, app, areas.input);
    if let Some(panel) = app.sessions() {
        sessions_card(frame, app, panel, areas.scrollback);
    }
    if app.browsing() {
        browse_card(frame, app, areas.scrollback);
    }
    if let Some(nick) = app.profile() {
        profile(frame, app, &app.chat.profile(nick), areas.scrollback);
    }
    if let Some((nick, session)) = app.agent() {
        agent_card(frame, app, nick, session, areas.scrollback);
    }
    if let Some((details, scrolled)) = app.details() {
        details_card(frame, app, details, scrolled, areas.scrollback);
    }
}

/// A message's details over the scrollback, `area`, as zirc-chat says them
/// ([`zirc_core::chat::Details`]): its title, a row a detail, then its data's heading over the
/// JSON, scrolled past `scrolled` lines.
fn details_card(
    frame: &mut Frame<'_>,
    app: &App,
    details: &zirc_core::chat::Details,
    scrolled: u16,
    area: Rect,
) {
    let theme = app.theme;
    let width = area.width.clamp(30, 96).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let bold = fg(theme.text).add_modifier(Modifier::BOLD);
    let muted = fg(theme.muted);
    let label_width = details
        .rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = vec![Line::styled(fit(&details.title, text), bold)];
    for row in &details.rows {
        lines.push(Line::from(vec![
            Span::styled(format!("{:label_width$}  ", row.label), muted),
            Span::styled(
                fit(&row.value, text.saturating_sub(label_width + 2)),
                fg(theme.text),
            ),
        ]));
    }
    if let Some(data) = &details.data {
        lines.push(Line::default());
        lines.push(Line::styled(fit(&details.data_heading, text), bold));
        lines.extend(
            data.lines()
                .map(|line| Line::styled(fit(line, text), fg(theme.text))),
        );
    }
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let most = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(height.saturating_sub(2));
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + u16::from(area.height > height),
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scrolled.min(most), 0))
            .block(panel_block(theme).padding(ratatui::widgets::Padding::horizontal(1))),
        card,
    );
}

/// The card of `nick`'s agent `session` over the scrollback, `area`, as zirc-chat says it
/// ([`zirc_core::chat::AgentCard`]): its name, whose it is and where, Now, the files it's
/// changing with others, each in zirc-chat's one phrase, and when it was last heard.
fn agent_card(frame: &mut Frame<'_>, app: &App, nick: &str, session: &str, area: Rect) {
    let theme = app.theme;
    let width = area.width.clamp(30, 72).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let muted = fg(theme.muted);
    let card = app.chat.agent_card(nick, session, (app.chat.clock)());
    let mut lines = vec![Line::styled(
        fit(&card.title, text),
        fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    let about = [card.whose, card.place, card.standing]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    lines.extend(
        wrap(&about, text)
            .into_iter()
            .map(|line| Line::styled(line, muted)),
    );
    if let Some(now) = card.now {
        lines.push(Line::default());
        lines.push(Line::styled(
            Type::get().label(zirc_core::chat::labels::tui::NOW),
            muted,
        ));
        lines.extend(
            wrap(&now, text)
                .into_iter()
                .map(|line| Line::styled(line, fg(theme.text))),
        );
    }
    let both = app
        .chat
        .both_changing_card(&app.chat.both_changing_for(session), session);
    if !both.lines.is_empty() {
        lines.push(Line::default());
    }
    for parts in &both.lines {
        let said = format!("{} {} {}", parts.verb, parts.paths.join(" "), parts.details);
        let glyph = parts.icon.glyph();
        for (i, line) in wrap(&said, text.saturating_sub(glyph.width() + 1))
            .into_iter()
            .enumerate()
        {
            let mark = if i == 0 { glyph } else { " " };
            lines.push(Line::from(vec![
                Span::styled(format!("{mark} "), icon_style(app, parts.icon)),
                Span::styled(line, fg(theme.text)),
            ]));
        }
    }
    if let Some(more) = both.more {
        lines.push(Line::styled(more, muted));
    }
    if let Some(heard) = card.heard {
        lines.push(Line::default());
        lines.push(Line::styled(fit(&heard, text), muted));
    }
    lines.push(Line::default());
    lines.push(Line::styled(zirc_core::chat::labels::ESC_CLOSES, muted));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let at = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + u16::from(area.height > height),
        width,
        height,
    };
    // The name it goes by on the border, when its title says something else.
    let mut block = panel_block(theme).padding(ratatui::widgets::Padding::horizontal(1));
    if card.handle != card.title {
        block = block.title(format!(" {} ", fit(&card.handle, text)));
    }
    frame.render_widget(Clear, at);
    frame.render_widget(Paragraph::new(lines).block(block), at);
}

/// `profile` as a card over the top of the scrollback, `area`.
/// The sessions panel over the scrollback, `area`: each session's lines, from zirc-core's
/// [`zirc_core::sessions::Session::lines`], its name in its colour.
fn sessions_card(frame: &mut Frame<'_>, app: &App, panel: &SessionsPanel, area: Rect) {
    let theme = app.theme;
    let width = area.width.clamp(30, 100).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let mut lines: Vec<Line<'static>> = match &panel.listed {
        None => vec![Line::styled(
            zirc_core::chat::labels::tui::ASKING_ZIRCD,
            fg(theme.muted),
        )],
        Some(Err(why)) => vec![Line::styled(
            zirc_core::chat::labels::tui::zircd_said(why),
            fg(theme.error),
        )],
        Some(Ok(listed)) if listed.is_empty() => vec![empty_hint(
            theme,
            &fit(
                "No sessions in the last day: an agent's zirc watch --session shows here",
                text,
            ),
            0,
        )],
        Some(Ok(listed)) => listed
            .iter()
            .flat_map(|session| {
                session
                    .lines(panel.at)
                    .into_iter()
                    .enumerate()
                    .map(|(i, line)| {
                        let line = fit(&line, text);
                        if i == 0 {
                            Line::styled(line, nick_style(app, &session.name))
                        } else {
                            Line::styled(line, fg(theme.text))
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
    };
    lines.push(Line::styled(
        zirc_core::chat::labels::ESC_CLOSES,
        fg(theme.muted),
    ));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel_block(theme)
                .title(format!(" {} ", zirc_core::chat::labels::tui::SESSIONS))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        card,
    );
}

/// What Browse channels is headed, and says when there's nothing to join, as every app
/// words them.
const BROWSE_HEADING: &str = zirc_core::chat::BROWSE_CHANNELS;
const BROWSE_EMPTY: &str = zirc_core::chat::NO_OTHER_CHANNELS;

/// Browse channels over the scrollback, `area`: the channels we aren't in, as zirc-chat's
/// `Chat::browse` lists them, `#` muted, each with how many are in it, none for one nobody
/// is in; then how to join one.
fn browse_card(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let channels = app.chat.browse();
    let width = area.width.clamp(30, 72).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let widest = channels
        .iter()
        .map(|(name, _)| name.width())
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line<'static>> = channels
        .iter()
        .map(|(name, count)| {
            let mut spans = vec![Span::styled(fit(name, text), fg(theme.text))];
            if *count > 0 {
                spans.push(Span::styled(
                    format!(
                        "{}  {}",
                        " ".repeat(widest.saturating_sub(name.width())),
                        members_label(*count)
                    ),
                    fg(theme.muted),
                ));
            }
            Line::from(spans)
        })
        .collect();
    if lines.is_empty() {
        lines.push(empty_hint(theme, BROWSE_EMPTY, 0));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        fit(
            &format!(
                "/join #name joins, Tab completes · {}",
                zirc_core::chat::labels::ESC_CLOSES
            ),
            text,
        ),
        fg(theme.muted).add_modifier(Modifier::DIM),
    ));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel_block(theme)
                .title(format!(" {BROWSE_HEADING} "))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        card,
    );
}

/// A card's block over the scrollback, in the theme's colours.
fn panel_block(theme: &Theme) -> Block<'static> {
    panel(Block::new().borders(Borders::ALL), theme).border_style(fg(theme.signal))
}

fn profile(frame: &mut Frame<'_>, app: &App, profile: &Profile, area: Rect) {
    let theme = app.theme;
    let width = area.width.clamp(30, 72).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let lines = profile_lines(app, profile, text);
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + u16::from(area.height > height),
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel(Block::new().borders(Borders::ALL), theme)
                .border_style(fg(theme.signal))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        card,
    );
}

/// What a profile card says ([`Profile::card`]), each line at most `width` wide.
fn profile_lines(app: &App, profile: &Profile, width: usize) -> Vec<Line<'static>> {
    let theme = app.theme;
    let now = (app.chat.clock)();
    let muted = fg(theme.muted);
    let line = |text: String, style: Style| Line::styled(fit(&text, width), style);
    let me = profile.nick.eq_ignore_ascii_case(app.nick());
    let card = profile.card(now, |at| app.zone.wall(at));

    let role = match card.presence {
        Presence::Here => theme.ok,
        Presence::Away => theme.warning,
        Presence::Gone => theme.muted,
    };
    let state = match card.standing {
        label if label.is_empty() => label,
        label => format!(" {label}"),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(card.title, fg(theme.text).add_modifier(Modifier::BOLD)),
        Span::styled(fit(&state, width), fg(role)),
    ])];
    lines.extend(card.about.into_iter().map(|text| line(text, muted)));
    for work in card.work {
        lines.push(Line::default());
        lines.push(Line::styled(Type::get().label(&work.heading), muted));
        lines.extend(
            work.lines
                .into_iter()
                .map(|text| line(text, fg(theme.text))),
        );
    }
    lines.push(Line::default());
    let keys = if me {
        zirc_core::chat::labels::ESC_CLOSES.to_owned()
    } else {
        format!(
            "Enter talks to them · {}",
            zirc_core::chat::labels::ESC_CLOSES
        )
    };
    lines.push(line(keys, muted));
    lines
}

fn header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let buffer = app.current();
    let chat = &app.chat;
    let name = if buffer.kind == Kind::Channel {
        theme.channel
    } else {
        theme.text
    };
    let mut spans = vec![Span::raw(" ")];
    // Who it's with in front: an agent session by its tool's mark, a person by their
    // presence; a channel by its whole name, "#" and all.
    spans.extend(header_face(app, &buffer.name));
    // What every app's header calls it, and tags it with ([`Chat::header_title`]).
    spans.push(Span::styled(
        chat.header_title(&buffer.name),
        fg(name).add_modifier(Modifier::BOLD),
    ));
    if chat.shows_agent_tag(&buffer.name) {
        spans.push(Span::styled(
            format!(" {AGENT_TAG}"),
            fg(theme.muted).add_modifier(Modifier::BOLD),
        ));
    }
    // The core's one line under the name, as every app's header has it
    // ([`Chat::header_line`]): an agent at work after the here dot, a person away in the
    // warning colour, the rest muted.
    if let Some(line) = chat.header_line(&buffer.name) {
        spans.push(Span::raw("  "));
        if line.kind == HeaderLineKind::AgentAtWork {
            spans.push(presence_mark(theme, Presence::Here));
        }
        let tone = if line.kind == HeaderLineKind::Away {
            theme.warning
        } else {
            theme.muted
        };
        spans.push(Span::styled(line.text, fg(tone)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(fg(theme.text)),
        area,
    );
}

/// What heads the title line before a conversation's name: an agent session's tool mark in
/// its colour, or the presence of the person a direct message is with.
fn header_face(app: &App, name: &str) -> Vec<Span<'static>> {
    if let Some(Avatar::Agent { tool, hue, .. }) = app.chat.agent(name).map(|agent| agent.avatar) {
        let mark = zirc_core::chat::tool_mark(tool.as_deref());
        return vec![Span::styled(
            format!("{mark} "),
            fg_colour(hue_colour(app, hue)),
        )];
    }
    app.chat
        .direct_rows()
        .into_iter()
        .find(|row| row.name == name)
        .and_then(|row| row.presence)
        .map(|presence| presence_mark(app.theme, presence))
        .into_iter()
        .collect()
}

fn sidebar_width(app: &App) -> u16 {
    // Both halves of the switch, so it keeps its width when the other is shown.
    let widest = app
        .sidebar()
        .into_iter()
        .chain(app.sides(None))
        .collect::<Vec<_>>()
        .iter()
        .map(|row| match row {
            // The org is cut to fit, and the switch fits the width the rows want, its halves
            // cut to fit.
            Side::Org(_) | Side::Tabs(_) | Side::Empty { .. } => 0,
            Side::Heading { label, .. } => label.width() + FOLDED_MARK.width(),
            // An agent at work wants room for some of its task, which is cut to fit.
            Side::Row {
                label,
                depth,
                agent,
                presence,
                note,
                ..
            } => {
                label.width()
                    + INDENT * depth
                    + agent.map_or(0, |_| AGENT_MARK)
                    + presence.map_or(0, |_| PRESENCE_MARK)
                    + note.as_ref().map_or(0, |_| 4)
            }
        })
        .max()
        .unwrap_or(0);
    // A space, the name, room for a count, and the border; at least as wide as the list was
    // while Browse channels ended its channels, so short names still leave room for what was
    // said last.
    u16::try_from(widest + 7).unwrap_or(u16::MAX).clamp(22, 26)
}

fn sidebar_block() -> Block<'static> {
    Block::new().borders(Borders::RIGHT)
}

/// A side panel's block, on the terminal's background with a rule between.
fn side(block: Block<'static>, theme: &Theme) -> Block<'static> {
    block.border_style(fg(theme.rule)).style(fg(theme.text))
}

/// A card's block, over the scrollback: on the panel's colour, so it hides what's under it.
fn panel(block: Block<'static>, theme: &Theme) -> Block<'static> {
    side(block, theme).style(fg(theme.text).bg(colour(theme.panel)))
}

fn sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let block = side(sidebar_block(), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width);
    let lines: Vec<Line<'_>> = app
        .sidebar()
        .into_iter()
        .map(|row| stale(app, side_line(app, row, width)))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `line` as last known, while not connected ([`zirc_core::chat::Chat::is_stale`]):
/// quieter, never emptied, as every app dims what it can't vouch for. The selected row,
/// on its ground, keeps muted text undimmed, so where you are still shows.
fn stale(app: &App, line: Line<'static>) -> Line<'static> {
    if !app.chat.is_stale() {
        return line;
    }
    let selected = line.style.bg.is_some();
    let quiet = if selected {
        fg(app.theme.muted)
    } else {
        fg(app.theme.muted).add_modifier(Modifier::DIM)
    };
    let spans = line
        .spans
        .into_iter()
        .map(|span| {
            let style = span.style.patch(quiet).remove_modifier(Modifier::BOLD);
            Span::styled(span.content, style)
        })
        .collect::<Vec<_>>();
    Line::from(spans).style(line.style)
}

/// After a section's heading folded away: there's more in it than it shows.
const FOLDED_MARK: &str = " ▸";

/// The columns an agent session's mark takes in the list down the left, with its space.
const AGENT_MARK: usize = 2;

/// The fewest columns of what was said last worth showing after a name in the list.
const PREVIEW_MIN: usize = 6;

/// How far each step of nesting in the list down the left goes in.
const INDENT: usize = 2;

/// The list's first row, as Discord heads a server's: the organisation's name, and how many
/// people are in it, `Acme · 5 members`.
fn org_line(app: &App, org: &OrgHeader, width: usize) -> Line<'static> {
    let theme = app.theme;
    let count = format!(" · {}", org.members_label);
    let name = fit(&org.name, width.saturating_sub(1));
    let count = fit(&count, width.saturating_sub(1 + name.width()));
    Line::from(vec![
        Span::styled(
            format!(" {name}"),
            fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(count, fg(theme.muted)),
    ])
}

/// A part of the switch where its word doesn't fit: `#` for Channels, an envelope for
/// Messages.
fn tab_glyph(section: zirc_core::chat::SidebarSection) -> &'static str {
    match section {
        zirc_core::chat::SidebarSection::Channels => "#",
        zirc_core::chat::SidebarSection::Direct => "✉",
    }
}

/// The `Channels | Messages` switch, `width` columns wide, each half centred in its own:
/// the one shown reversed, the other bold while something in it is unread, with its count.
fn tabs_line(app: &App, tabs: &[zirc_core::chat::SidebarTab], width: usize) -> Line<'static> {
    let theme = app.theme;
    let halves = tabs.len().max(1);
    let spans = tabs.iter().enumerate().map(|(at, tab)| {
        // The last half takes what's left over.
        let room = if at + 1 == halves {
            width - width / halves * (halves - 1)
        } else {
            width / halves
        };
        let count = tab
            .badge_label
            .as_ref()
            .filter(|_| !tab.chosen)
            .map_or_else(String::new, |count| format!(" {count}"));
        // Too narrow for its word, its sign.
        let said = format!("{}{count}", tab.label);
        let words = if said.width() <= room {
            said
        } else {
            fit(&format!("{}{count}", tab_glyph(tab.section)), room)
        };
        let left = room.saturating_sub(words.width()) / 2;
        let right = room.saturating_sub(words.width() + left);
        let text = format!("{}{words}{}", " ".repeat(left), " ".repeat(right));
        let style = if tab.chosen {
            fg(theme.text).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if tab.unread {
            fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            fg(theme.muted)
        };
        Span::styled(text, style)
    });
    Line::from(spans.collect::<Vec<_>>())
}

/// A tag's words, `NEW`, in the case the design gives tags.
fn tag_text(text: &str) -> String {
    if Design::get().tag_case == "uppercase" {
        text.to_uppercase()
    } else {
        text.to_owned()
    }
}

/// A section's label, `Channels`, in the case the design gives section labels.
fn section_label(text: &str) -> String {
    if Design::get().section_label_case == "uppercase" {
        text.to_uppercase()
    } else {
        text.to_owned()
    }
}

/// An empty list's one line, `hint`, `indent` columns in: what it would hold and how to
/// start it, dim, as every app's empty list says it.
fn empty_hint(theme: &Theme, hint: &str, indent: usize) -> Line<'static> {
    Line::styled(
        format!("{}{hint}", " ".repeat(indent)),
        fg(theme.muted).add_modifier(Modifier::DIM),
    )
}

/// What was said last and when, as a list row draws them in the `room` its name leaves:
/// the words after a space, padded, and the time with a space before a count, if it's
/// `counted`; or just the padding, when they don't fit.
fn preview_columns(
    preview: Option<(String, String)>,
    room: usize,
    counted: bool,
) -> (String, String) {
    preview
        .and_then(|(said, when)| {
            let tail = when.width() + usize::from(counted);
            let fits = room
                .checked_sub(tail + 2)
                .filter(|&left| left >= PREVIEW_MIN)?;
            let said = fit(&said, fits);
            let gap = room - said.width() - tail - 1;
            let space = if counted { " " } else { "" };
            Some((
                format!(" {said}{}", " ".repeat(gap)),
                format!("{when}{space}"),
            ))
        })
        .unwrap_or_else(|| (" ".repeat(room), String::new()))
}

/// What an agent at work is taking on, as a list row draws it in the `room` its name
/// leaves, in place of what was said: after a dot, cut to fit, padded.
fn note_columns(note: &str, room: usize) -> (String, String) {
    let said = fit(&format!(" · {note}"), room);
    let gap = room.saturating_sub(said.width());
    (format!("{said}{}", " ".repeat(gap)), String::new())
}

/// What was said last in a list row, muted; zirc-chat's `No messages yet` in its stead,
/// faint.
fn said_style(theme: &Theme, placeholder: bool) -> Style {
    if placeholder {
        fg(theme.faint)
    } else {
        fg(theme.muted)
    }
}

/// A row of the list down the left, `width` columns wide.
fn side_line(app: &App, row: Side, width: usize) -> Line<'static> {
    match row {
        Side::Org(org) => org_line(app, &org, width),
        Side::Tabs(tabs) => tabs_line(app, &tabs, width),
        // The half's empty line, muted, and its hint faint under it, cut to fit.
        Side::Empty { text, hint } => Line::styled(
            format!(" {}", fit(&text, width.saturating_sub(1))),
            fg(if hint {
                app.theme.faint
            } else {
                app.theme.muted
            }),
        ),
        // Folded away, a mark after it says there's more under it.
        Side::Heading { label, open, .. } => Line::styled(
            format!(
                " {}{}",
                section_label(label),
                if open { "" } else { FOLDED_MARK }
            ),
            fg(app.theme.muted).add_modifier(Modifier::BOLD),
        ),
        row @ Side::Row { .. } => side_row_line(app, row, width),
    }
}

/// A conversation's row, or a fold's, in the list down the left, `width` columns wide.
fn side_row_line(app: &App, row: Side, width: usize) -> Line<'static> {
    let theme = app.theme;
    let Side::Row {
        label,
        target,
        depth,
        pip,
        agent,
        preview,
        placeholder,
        note,
        presence,
        bold,
        badge,
    } = row
    else {
        return Line::default();
    };
    // An agent session: its tool's mark in its colour, in front of what it goes by.
    let indent = " ".repeat(INDENT * depth);
    let lead = |indent: &str| {
        let mut spans = vec![Span::raw(indent.to_owned())];
        if let Some((mark, hue)) = agent {
            spans.push(Span::styled(
                format!("{mark} "),
                fg_colour(hue_colour(app, hue)),
            ));
        }
        spans
    };
    // What's drawn before the label, and how wide that all is.
    let (mut spans, mut indent, mut lead_width) = match agent {
        Some(_) => (lead(&indent), String::new(), indent.width() + AGENT_MARK),
        None => (Vec::new(), indent.clone(), indent.width()),
    };
    // The left edge, where an unread row's pip goes.
    spans.insert(0, Span::raw(" "));
    lead_width += 1;
    // A person: their presence in front, as zirc-chat says it.
    if let Some(presence) = presence {
        spans.push(Span::raw(std::mem::take(&mut indent)));
        spans.push(presence_mark(theme, presence));
        lead_width += PRESENCE_MARK;
    }
    // A row gathering others, or an agent session's not yet opened, with what it's taking
    // on after it.
    let Some(Target::Conversation(at)) = target else {
        let room = width.saturating_sub(lead_width + 1);
        let label = fit(&label, room);
        let (note, _) = note
            .map(|note| note_columns(&note, room - label.width()))
            .unzip();
        spans.push(Span::styled(format!("{indent}{label}"), fg(theme.muted)));
        spans.extend(note.map(|note| Span::styled(note, fg(theme.muted))));
        return Line::from(spans);
    };
    // A pill for what zirc-chat counts: mentions in a channel, everything in a DM.
    let count = match badge {
        0 => String::new(),
        badge => format!(" {} ", zirc_core::chat::badge_label(badge as usize)),
    };
    // A starred row isn't marked, as in every app: it's under Starred (the user's choice,
    // 20 Nov 2026).
    let name = fit(&label, width.saturating_sub(lead_width + count.width() + 1));
    // What was said last, dimmed after the name, and when, at the right, where there's room.
    let room = width.saturating_sub(lead_width + name.width() + count.width() + 1);
    // The pill pads itself, so the time needs no space before it.
    let said_style = said_style(theme, placeholder);
    let (said, when) = match note {
        Some(note) => note_columns(&note, room),
        None => preview_columns(preview, room, false),
    };
    // Unread in bold text with a pip at the edge, read quieter; muted never stands out
    // (`ListRow::bold`).
    let unread = bold;
    let selected = at == app.selected();
    let name_style = if unread || selected {
        fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        fg(theme.muted)
    };
    let count_style = fg(theme.on_badge).bg(colour(theme.badge));
    if pip {
        spans[0] = Span::styled("▎", fg(theme.text));
    }
    spans.push(Span::styled(indent, name_style));
    spans.extend([
        Span::styled(name, name_style),
        Span::styled(said, said_style),
        Span::styled(when, said_style),
        Span::styled(count, count_style),
        // A column's margin at the right edge.
        Span::raw(" "),
    ]);
    let line = Line::from(spans);
    if selected {
        line.style(Style::new().bg(colour(theme.selected)))
    } else {
        line
    }
}

/// The rows the scrollback, `area`, shows from the top, `None` for the blank ones above the
/// oldest.
fn shown_rows(app: &App, area: Rect) -> Vec<Option<Row>> {
    app.shown_rows(usize::from(area.width), usize::from(area.height))
}

fn scrollback(frame: &mut Frame<'_>, app: &App, area: Rect) {
    scrollback_rows(frame, app, area);
    // Nothing said yet: how it starts, as every app says it, near the top.
    // The server's own conversation, its notices, has no start to tell.
    let intro = (app.current().kind != Kind::Server)
        .then(|| app.chat.intro(&app.current().name))
        .flatten();
    if let Some(intro) = intro {
        intro_card(frame, app, area, &intro);
    }
    // Not connected: one warning line over the top, saying so; what's shown stays.
    if let Some(banner) = offline_banner(app) {
        let row = Rect { height: 1, ..area };
        frame.render_widget(Paragraph::new(banner), row);
    }
}

/// What's new about the connection, over the scrollback, while it isn't up, as zirc-core
/// words it for every app ([`zirc_core::connection::Status::banner`]): `Reconnecting…`,
/// then `Offline · retrying in 3s`, on the warning colour's ground.
fn offline_banner(app: &App) -> Option<Line<'static>> {
    let theme = app.theme;
    let words = app.connection.banner((app.chat.clock)())?.text;
    Some(
        Line::styled(
            format!(" {words}"),
            fg(theme.warning).add_modifier(Modifier::BOLD),
        )
        .style(Style::new().bg(colour(theme.mention_ground))),
    )
}

/// A conversation's start, `intro`'s lines, from the text column near the top: the first
/// as a title, the topic under it quieter.
fn intro_card(frame: &mut Frame<'_>, app: &App, area: Rect, intro: &[String]) {
    let theme = app.theme;
    let column = " ".repeat(rows::INDENT);
    let width = usize::from(area.width).saturating_sub(rows::INDENT);
    let lines: Vec<Line<'_>> = intro
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let style = if i == 0 {
                fg(theme.text).add_modifier(Modifier::BOLD)
            } else {
                fg(theme.muted)
            };
            Line::styled(format!("{column}{}", fit(text, width)), style)
        })
        .collect();
    let top = area.height.min(2);
    let at = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    frame.render_widget(Paragraph::new(lines), at);
}

fn scrollback_rows(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let shown = shown_rows(app, area);
    let lines: Vec<Line<'_>> = shown
        .iter()
        .map(|row| {
            row.as_ref()
                .map_or_else(|| Line::raw(""), |row| row_line(app, row, area.width))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
    // Links, clickable where the terminal can: see [`crate::hyperlink`].
    for (y, row) in (area.y..).zip(&shown) {
        if let Some(Row::Text { links, .. } | Row::System { links, .. }) = row {
            // People and agent sessions named in it as mention chips, with no "@".
            let mention = fg(app.theme.mention_chip_text)
                .bg(colour(app.theme.mention_chip_ground))
                .add_modifier(Modifier::BOLD);
            hyperlink::mark(frame.buffer_mut(), area, y, links, mention);
        }
    }
    if let Some(picker) = app.picker() {
        picker_card(frame, app, picker, area);
    }
    if let Some(jumping) = app.jumping() {
        jump_card(frame, app, jumping, area);
    }
    if let Some(searching) = app.searching() {
        search_card(frame, app, (searching, searching.screen()), area);
    }
    if let Some(trimming) = app.trimming() {
        trim_card(frame, app, trimming, area);
    }
}

/// Asks whether to trim a message, as a card over the top of the scrollback, `area`,
/// saying which and what trimming reaches.
fn trim_card(frame: &mut Frame<'_>, app: &App, trimming: &Trimming, area: Rect) {
    let theme = app.theme;
    let width = area.width.clamp(30, 72).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let line = |words: &str, style: Style| Line::styled(fit(words, text), style);
    let mut lines = vec![line(&trimming.text, fg(theme.text))];
    if let Some(reason) = &trimming.reason {
        lines.push(line(&format!("because: {reason}"), fg(theme.muted)));
    }
    lines.push(Line::default());
    for words in [zirc_core::trim::reach(), zirc_core::trim::KEPT.to_owned()] {
        lines.push(line(&words, fg(theme.muted)));
    }
    lines.push(Line::default());
    lines.push(line(
        "y trims it · any other key keeps it",
        fg(theme.warning),
    ));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let card = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel(Block::new().borders(Borders::ALL), theme)
                .border_style(fg(theme.warning))
                .title(format!(" {} ", zirc_core::chat::labels::tui::TRIM_THIS))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        card,
    );
}

/// What's at `column` in the scrollback's `row`, `area`: a reaction, a message or neither.
fn scrollback_target(app: &App, area: Rect, column: u16, row: u16) -> Target {
    let shown = shown_rows(app, area);
    let x = usize::from(column - area.x);
    match shown.get(usize::from(row - area.y)).cloned().flatten() {
        Some(Row::Reactions { id, chips }) => x
            .checked_sub(rows::INDENT)
            .and_then(|x| rows::chip_at(&chips, x))
            .map_or(Target::Message(id.clone()), |chip| Target::Reaction {
                id: id.clone(),
                emoji: chip.emoji.clone(),
            }),
        Some(
            Row::Text { id: Some(id), .. }
            | Row::Header { id: Some(id), .. }
            | Row::System { id: Some(id), .. },
        ) => Target::Message(id),
        _ => Target::Scrollback,
    }
}

/// One line of search's card, with the row it's part of (a hit, or a recent search), to keep
/// the one picked in view.
type Listed = (Line<'static>, Option<usize>);

/// Search, as a card over the top of the scrollback, `area`, laid out as zirc-chat lays it
/// out for every app ([`zirc_core::chat::Chat::search_screen`]): the box with its chips,
/// here or everywhere, what it couldn't cover, then the recent searches while nothing's
/// typed, else what it found, each hit's who and when over its words with the matches bold.
fn search_card(
    frame: &mut Frame<'_>,
    app: &App,
    (searching, screen): (&Searching, &SearchScreen),
    area: Rect,
) {
    use zirc_core::chat::labels;
    let theme = app.theme;
    let width = area.width.clamp(30, 80).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let head = search_head(app, (searching, screen), text);
    let recent = app.recent_searches();
    let (body, at) = if searching.blank() {
        let at = searching.at(recent.len());
        (recent_lines(app, &recent, at, text), at)
    } else {
        let at = searching.at(screen.listed_hits().len());
        (found_lines(app, screen, at, text), at)
    };
    let mut keys = vec![labels::tui::ENTER_OPENS];
    if searching.finding.opened_in.is_some() {
        keys.push(labels::tui::TAB_EVERYWHERE);
    }
    keys.push(labels::ESC_CLOSES);
    let foot = Line::styled(fit(&keys.join(" · "), text), fg(theme.muted));
    // The rows that fit between the head and the foot, round the one picked.
    let room = usize::from(area.height.saturating_sub(2))
        .saturating_sub(head.len() + 1)
        .max(1);
    let last = body
        .iter()
        .rposition(|(_, row)| *row == Some(at))
        .unwrap_or(0);
    let first = (last + 1).saturating_sub(room);
    let mut lines = head;
    lines.extend(
        body.into_iter()
            .skip(first)
            .take(room)
            .map(|(line, _)| line),
    );
    lines.push(foot);
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let card = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel(Block::new().borders(Borders::ALL), theme)
                .border_style(fg(theme.signal))
                .title(format!(
                    " {} ",
                    labels::with_shortcut(zirc_core::chat::SEARCH, labels::tui::SEARCH_KEY)
                )),
        ),
        card,
    );
}

/// `line` on the signal colour when it's part of the row picked.
fn picked_line(app: &App, line: Line<'static>, picked: bool) -> Line<'static> {
    if picked {
        line.style(fg(app.theme.on_signal).bg(colour(app.theme.signal)))
    } else {
        line
    }
}

/// Search's box, its chips then what's typed, or what it says while empty; here or
/// everywhere; and why what's typed doesn't read, or what it couldn't cover.
fn search_head(
    app: &App,
    (searching, screen): (&Searching, &SearchScreen),
    text: usize,
) -> Vec<Line<'static>> {
    let theme = app.theme;
    let finding = &searching.finding;
    let mut spans: Vec<Span<'static>> = Vec::new();
    for chip in &screen.chips {
        spans.push(Span::styled(
            format!(" {} ", chip.label),
            fg(theme.text).bg(colour(theme.panel)),
        ));
        spans.push(Span::raw(" "));
    }
    if finding.typed.is_empty() && screen.chips.is_empty() {
        spans.push(Span::styled(screen.placeholder.clone(), fg(theme.muted)));
    }
    spans.push(Span::styled(finding.typed.clone(), fg(theme.text)));
    spans.push(Span::styled("▏", fg(theme.signal)));
    let mut head = vec![Line::from(spans)];
    let here = finding
        .opened_in
        .as_deref()
        .map(|name| app.chat.title(name));
    if let Some(segments) = finding.segments(screen, here) {
        let chosen = fg(theme.text).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let (here, everywhere) = if segments.everywhere_chosen {
            (fg(theme.muted), chosen)
        } else {
            (chosen, fg(theme.muted))
        };
        head.push(Line::from(vec![
            Span::styled(segments.here, here),
            Span::styled(" · ", fg(theme.muted)),
            Span::styled(segments.everywhere, everywhere),
        ]));
    }
    let warning = |words: &str| Line::styled(fit(words, text), fg(theme.warning));
    head.extend(finding.problem().as_deref().map(warning));
    if let Some(limits) = &screen.limits {
        head.push(if limits.warning {
            warning(&limits.text)
        } else {
            Line::styled(fit(&limits.text, text), fg(theme.muted))
        });
    }
    head
}

/// The recent searches, as the empty box lists them, the one `at` picked.
fn recent_lines(
    app: &App,
    recent: &[zirc_core::chat::Recent],
    at: usize,
    text: usize,
) -> Vec<Listed> {
    let theme = app.theme;
    let mut body = Vec::new();
    if !recent.is_empty() {
        body.push((
            Line::styled(
                Type::get().label(zirc_core::chat::RECENT_TITLE),
                fg(theme.muted),
            ),
            None,
        ));
    }
    for (i, search) in recent.iter().enumerate() {
        let said = search
            .chips
            .iter()
            .cloned()
            .chain((!search.text.is_empty()).then(|| search.text.clone()))
            .collect::<Vec<_>>()
            .join(" ");
        let line = Line::styled(fit(&said, text), fg(theme.text));
        body.push((picked_line(app, line, i == at), Some(i)));
    }
    body
}

/// What `screen` found: its header, what it says when nothing matches, then each hit, in
/// one list or under its conversation, the one `at` picked.
fn found_lines(app: &App, screen: &SearchScreen, at: usize, text: usize) -> Vec<Listed> {
    let theme = app.theme;
    let muted = |words: &str| (Line::styled(fit(words, text), fg(theme.muted)), None);
    let mut body: Vec<Listed> = screen.header.iter().map(|header| muted(header)).collect();
    if let Some(empty) = &screen.empty {
        body.push((Line::styled(fit(&empty.text, text), fg(theme.text)), None));
        body.extend(empty.hint.iter().map(|hint| muted(hint)));
    }
    let mut i = 0;
    let mut hit = |body: &mut Vec<Listed>, hit: &SearchHit| {
        for line in hit_lines(app, hit, text) {
            body.push((picked_line(app, line, i == at), Some(i)));
        }
        i += 1;
    };
    if screen.groups.is_empty() {
        for found in &screen.hits {
            hit(&mut body, found);
        }
    } else {
        for group in &screen.groups {
            let title = fit(&group.title, text);
            body.push((
                Line::styled(title, fg(theme.muted).add_modifier(Modifier::BOLD)),
                None,
            ));
            for found in &group.hits {
                hit(&mut body, found);
            }
        }
    }
    body.extend(screen.more_groups.iter().map(|more| muted(more)));
    body
}

/// A hit's two lines: who said it and when, then its words, the matches bold.
fn hit_lines(app: &App, hit: &SearchHit, text: usize) -> [Line<'static>; 2] {
    let theme = app.theme;
    let who = Line::from(vec![
        Span::styled(
            hit.name.clone(),
            fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", hit.aside), fg(theme.muted)),
    ]);
    let mut spans = Vec::new();
    let mut room = text;
    for part in &hit.parts {
        if room == 0 {
            break;
        }
        let shown = fit(&part.text, room);
        room = room.saturating_sub(shown.width());
        let style = if part.hit {
            fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            fg(theme.text)
        };
        spans.push(Span::styled(shown, style));
    }
    [who, Line::from(spans)]
}

/// Jump to, as a card over the top of the scrollback, `area`, laid out as zirc-chat lays it
/// out for every app ([`zirc_core::chat::Chat::jump_to`]): what's typed, `Nothing called
/// “x”.` when it offers nothing, the conversations it finds by title and mention badge (the
/// user's choice, 20 Nov 2026), Browse channels' heading over the channels to join with how
/// many are in each, then Join or Create #name.
fn jump_card(frame: &mut Frame<'_>, app: &App, jumping: &Jumping, area: Rect) {
    use zirc_core::chat::labels;
    let theme = app.theme;
    let jump = app.chat.jump_to(&jumping.query);
    let at = jumping.at(jump::names(&jump).len());
    let width = area.width.clamp(30, 60).min(area.width);
    let text = usize::from(width).saturating_sub(4);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", zirc_core::chat::JUMP_TO_PROMPT),
            fg(theme.muted),
        ),
        Span::styled(jumping.query.clone(), fg(theme.text)),
        Span::styled("▏", fg(theme.signal)),
    ])];
    if let Some(nothing) = &jump.nothing {
        lines.push(Line::styled(fit(nothing, text), fg(theme.muted)));
    }
    // Each row to go to, the one picked on the signal colour.
    let mut row = 0;
    let mut push = |lines: &mut Vec<Line<'static>>, spans: Vec<Span<'static>>| {
        let line = Line::from(spans);
        lines.push(if row == at {
            line.style(fg(theme.on_signal).bg(colour(theme.signal)))
        } else {
            line
        });
        row += 1;
    };
    for row in &jump.conversations {
        let badge = row
            .badge
            .as_ref()
            .map(|badge| format!(" {badge} "))
            .unwrap_or_default();
        let title = fit(&row.title, text.saturating_sub(badge.width() + 1));
        push(
            &mut lines,
            vec![
                Span::styled(title, fg(theme.text)),
                Span::raw(" "),
                Span::styled(badge, fg(theme.on_signal).bg(colour(theme.mention_edge))),
            ],
        );
    }
    if let Some(heading) = &jump.browse_heading {
        lines.push(Line::styled(Type::get().label(heading), fg(theme.muted)));
    }
    for channel in &jump.browse {
        let members = channel
            .members
            .as_ref()
            .map(|members| format!("  {members}"))
            .unwrap_or_default();
        push(
            &mut lines,
            vec![
                Span::styled(channel.name.clone(), fg(theme.channel)),
                Span::styled(members, fg(theme.muted)),
            ],
        );
    }
    if let Some(offer) = &jump.offer {
        push(
            &mut lines,
            vec![Span::styled(
                fit(&format!("{} {}", offer.mark, offer.label), text),
                fg(theme.text),
            )],
        );
    }
    lines.push(Line::styled(
        fit(
            &format!("{} · {}", labels::tui::ENTER_OPENS, labels::ESC_CLOSES),
            text,
        ),
        fg(theme.muted),
    ));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let card = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel(Block::new().borders(Borders::ALL), theme)
                .border_style(fg(theme.signal))
                .title(format!(
                    " {} ",
                    labels::with_shortcut(zirc_core::chat::JUMP_TO, labels::tui::JUMP_KEY)
                )),
        ),
        card,
    );
}

/// The reaction picker, as a card over the bottom of the scrollback, `area`.
fn picker_card(frame: &mut Frame<'_>, app: &App, picker: &Picker, area: Rect) {
    let theme = app.theme;
    let rows = picker.rows(&app.recent);
    let (at_row, at_column) = picker.at();
    // An emoji and a space each, and the border and padding.
    let width = u16::try_from(picker::PER_ROW * 3 + 4)
        .unwrap_or(u16::MAX)
        .min(area.width);
    let grid = usize::from(area.height.saturating_sub(5)).clamp(1, 8);
    // Rows shown: a window round the cursor's, from the top where it can.
    let first = at_row
        .saturating_sub(grid - 1)
        .min(rows.len().saturating_sub(grid));
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", zirc_core::chat::labels::tui::SEARCH),
            fg(theme.muted),
        ),
        Span::styled(picker.query.clone(), fg(theme.text)),
        Span::styled("▏", fg(theme.signal)),
    ])];
    for (i, row) in rows.iter().enumerate().skip(first).take(grid) {
        lines.push(match row {
            emoji::Row::Heading(section) => {
                Line::styled(Type::get().label(section.title()), fg(theme.muted))
            }
            emoji::Row::Emoji(emoji) => Line::from(
                emoji
                    .iter()
                    .enumerate()
                    .map(|(j, e)| {
                        let style = if (i, j) == (at_row, at_column) {
                            fg(theme.on_signal).bg(colour(theme.signal))
                        } else {
                            fg(theme.text)
                        };
                        Span::styled(format!("{e} "), style)
                    })
                    .collect::<Vec<_>>(),
            ),
        });
    }
    if rows.is_empty() {
        lines.push(empty_hint(theme, "Nothing found: type less, or Esc", 0));
    }
    let text = usize::from(width).saturating_sub(4);
    let chosen = picker
        .chosen(&app.recent)
        .and_then(emoji::name)
        .unwrap_or_default();
    lines.push(Line::styled(fit(chosen, text), fg(theme.text)));
    lines.push(Line::styled(
        fit(
            &format!("Enter reacts · {}", zirc_core::chat::labels::ESC_CLOSES),
            text,
        ),
        fg(theme.muted),
    ));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    // At the top, out of the way of the newest messages, which are what's reacted to most.
    let card = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(lines).block(
            panel(Block::new().borders(Borders::ALL), theme)
                .border_style(fg(theme.signal))
                .title(format!(" {} ", zirc_core::chat::labels::tui::REACT))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        card,
    );
}

/// `line` on the panel's colour when it's part of the message picked.
fn highlight(app: &App, line: Line<'static>, picked: bool) -> Line<'static> {
    if picked {
        line.style(Style::new().bg(colour(app.theme.panel)))
    } else {
        line
    }
}

/// The header of a run of what `nick` said: who, then when, then which app it was sent
/// from, dimmed.
fn header_line(
    app: &App,
    (nick, agent): (&str, Option<&rows::Agent>),
    time: &str,
    pill: Option<&str>,
    hint: &str,
) -> Line<'static> {
    let theme = app.theme;
    let style = nick_style(app, nick);
    let mut spans = Vec::new();
    // An agent session: its tool's mark in its own colour, its name, then whose it is.
    if let Some(agent) = agent {
        spans.push(Span::styled(
            format!("{} ", agent.mark),
            fg_colour(hue_colour(app, agent.hue)),
        ));
    }
    spans.push(Span::styled(
        nick.to_owned(),
        style.add_modifier(Modifier::BOLD),
    ));
    if let Some(agent) = agent.filter(|agent| !agent.detail.is_empty()) {
        spans.push(Span::styled(
            format!(" {}", agent.detail),
            fg(theme.muted).add_modifier(Modifier::DIM),
        ));
    }
    // An app's tag is quiet.
    if let Some(pill) = pill {
        spans.push(Span::styled(format!(" {pill}"), fg(theme.muted)));
    }
    spans.push(Span::styled(format!("  {time}"), fg(theme.muted)));
    if !hint.is_empty() {
        spans.push(Span::styled(
            format!("  {hint}"),
            fg(theme.muted).add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

fn row_line(app: &App, row: &Row, width: u16) -> Line<'static> {
    let theme = app.theme;
    let picked = |id: &Option<String>| id.is_some() && id.as_deref() == app.picked();
    match row {
        Row::Buttons { labels, id, said } => {
            highlight(app, buttons_line(app, labels, said.as_deref()), picked(id))
        }
        Row::Reactions { chips, id } => {
            highlight(app, reactions_line(app, chips), picked(&Some(id.clone())))
        }
        Row::Header {
            nick,
            time,
            hint,
            id,
            pill,
            agent,
        } => highlight(
            app,
            header_line(app, (nick, agent.as_ref()), time, *pill, hint),
            picked(id),
        ),
        Row::New => new_line(app, width),
        Row::System { id, .. } => highlight(app, system_line(app, row), picked(id)),
        Row::Divider(day) => {
            let label = format!(" {day} ");
            let side = usize::from(width).saturating_sub(label.width()) / 2;
            Line::styled(
                format!("{}{label}{}", "─".repeat(side), "─".repeat(side)),
                fg(theme.muted),
            )
        }
        Row::Text {
            nick,
            text,
            tone,
            mention,
            hint,
            id,
            ..
        } => {
            let (nick_style, text_style) = text_styles(app, nick, tone);
            // Under its header: a mark in front, the bar of the message picked, or the edge
            // of one that mentions us.
            let lead = if picked(id) && nick.is_empty() {
                Span::styled("▌", fg(theme.signal))
            } else if *mention && nick.is_empty() {
                Span::styled("▎", fg(theme.mention_edge))
            } else {
                Span::styled(nick.clone(), nick_style)
            };
            let pad = rows::INDENT.saturating_sub(lead.width());
            let mut spans = vec![lead, Span::raw(" ".repeat(pad))];
            spans.extend(lead_spans(app, text, tone, text_style));
            // The session an app's post is about, dim at the end.
            if !hint.is_empty() {
                spans.push(Span::styled(
                    format!(" {hint}"),
                    fg(theme.muted).add_modifier(Modifier::DIM),
                ));
            }
            mentioned(app, highlight(app, Line::from(spans), picked(id)), *mention)
        }
    }
}

/// A body row's `text` in `style`, but for an event's lead, whose marker, its first
/// character, is in the theme role zirc-chat names for it ([`Tone::Lead`]), and its details
/// after it on the row in theirs, as every app draws it.
fn lead_spans(app: &App, text: &str, tone: &Tone, style: Style) -> Vec<Span<'static>> {
    let Tone::Lead { marker, details } = tone else {
        return vec![Span::styled(text.to_owned(), style)];
    };
    let glyph = text.chars().next().map_or(0, char::len_utf8);
    let (words, details) = match details {
        Some((from, role)) if text.is_char_boundary(*from) && *from >= glyph => {
            (&text[glyph..*from], Some((&text[*from..], role)))
        }
        _ => (&text[glyph..], None),
    };
    let mut spans = vec![
        Span::styled(text[..glyph].to_owned(), role_style(app, marker)),
        Span::styled(words.to_owned(), style),
    ];
    if let Some((details, role)) = details {
        spans.push(Span::styled(details.to_owned(), role_style(app, role)));
    }
    spans
}

/// The colour of the theme role `role`, as zirc-chat names one, muted when the theme has none.
fn role_style(app: &App, role: &str) -> Style {
    let theme = app.theme;
    fg(theme.role(role).unwrap_or(theme.muted))
}

/// The shortest rule worth drawing before what's new.
const NEW_RULE_MIN: usize = 4;

/// Where what hadn't been read starts: a red line, `New` at its end, as Slack draws it.
fn new_line(app: &App, width: u16) -> Line<'static> {
    // How much is new, and since when, in zirc-chat's words; its short ones where the row
    // has no room for a rule before them.
    let label = app.chat.unseen(|at| app.zone.wall(at)).map_or_else(
        || format!(" {}", zirc_core::chat::NEW_TAG),
        |unseen| {
            let long = format!(" {}", unseen.label);
            if long.width() + NEW_RULE_MIN <= usize::from(width) {
                long
            } else {
                format!(" {}", unseen.short)
            }
        },
    );
    // The words, then a NEW tag on the badge colour, as every app ends the rule.
    let theme = app.theme;
    let tag = format!(" {} ", tag_text("new"));
    let line = "─".repeat(usize::from(width).saturating_sub(label.width() + 1 + tag.width()));
    Line::from(vec![
        Span::styled(format!("{line}{label} "), fg(theme.badge)),
        Span::styled(
            tag,
            fg(theme.on_badge)
                .bg(colour(theme.badge))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// A message's reactions, as chips from the text column.
fn reactions_line(app: &App, chips: &[rows::Chip]) -> Line<'static> {
    let theme = app.theme;
    let mut spans = vec![Span::raw(" ".repeat(rows::INDENT))];
    for (i, chip) in chips.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(rows::CHIP_GAP)));
        }
        let style = if chip.mine {
            fg(theme.reaction_mine_text).bg(colour(theme.reaction_mine_ground))
        } else {
            fg(theme.text).bg(colour(theme.panel))
        };
        spans.push(Span::styled(rows::chip_text(chip), style));
    }
    Line::from(spans)
}

/// A card's buttons, from the text column: the first a tonal word and the rest outlined (in
/// brackets), as every app draws them (zirc-chat's `CardAction::primary`); `/press` presses one.
fn buttons_line(app: &App, labels: &[String], said: Option<&str>) -> Line<'static> {
    let theme = app.theme;
    let mut spans = vec![Span::raw(" ".repeat(rows::INDENT))];
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(rows::CHIP_GAP)));
        }
        spans.push(if i == 0 {
            Span::styled(
                format!(" {label} "),
                fg(theme.tonal_text)
                    .bg(colour(theme.tonal_ground))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!("[{label}]"), fg(theme.text))
        });
    }
    // What came of our last press, after them, until the card changes.
    if let Some(said) = said {
        spans.push(Span::styled(format!("  {said}"), fg(theme.muted)));
    }
    Line::from(spans)
}

/// `line`, of a message that mentions us when `mention`, on the mention ground.
fn mentioned(app: &App, line: Line<'static>, mention: bool) -> Line<'static> {
    if mention && line.style.bg.is_none() {
        line.style(Style::new().bg(colour(app.theme.mention_ground)))
    } else {
        line
    }
}

/// How a row of text reads: the style of what's in its indent, and of its text.
fn text_styles(app: &App, nick: &str, tone: &Tone) -> (Style, Style) {
    let theme = app.theme;
    let nick_style = match tone {
        Tone::Message | Tone::Notice | Tone::App(None) => nick_style(app, nick),
        Tone::App(Some(icon)) => icon_style(app, *icon),
        _ => fg(theme.muted),
    };
    let text_style = match tone {
        Tone::Message | Tone::App(_) | Tone::Code => fg(theme.body),
        Tone::Action => fg(theme.body).add_modifier(Modifier::ITALIC),
        // A notice's words, muted as zirc-chat styles them; a lead's marker is in the role
        // zirc-chat names ([`lead_spans`]), its words muted.
        Tone::Notice | Tone::Lead { .. } | Tone::Status | Tone::Quote => fg(theme.muted),
        Tone::Details(role) => fg(theme.role(role).unwrap_or(theme.body)),
        Tone::Path | Tone::More => fg(theme.muted).add_modifier(Modifier::DIM),
        Tone::Heading => fg(theme.text).add_modifier(Modifier::BOLD),
    };
    (nick_style, text_style)
}

/// A quiet row: its agent's mark, when an agent did it, its marker, what it says, muted,
/// then when, dim, when it shows a time.
fn system_line(app: &App, row: &Row) -> Line<'static> {
    let Row::System {
        time,
        glyph,
        text,
        agent,
        mention,
        ..
    } = row
    else {
        return Line::raw("");
    };
    let theme = app.theme;
    let mut spans = Vec::new();
    if let Some(agent) = agent {
        spans.push(Span::styled(
            format!("{:1$}", agent.mark, rows::AGENT_MARK_WIDTH),
            fg_colour(hue_colour(app, agent.hue)),
        ));
    }
    spans.push(Span::styled(glyph.clone(), fg(theme.muted)));
    spans.push(Span::raw(
        " ".repeat(rows::INDENT.saturating_sub(glyph.width())),
    ));
    // One that mentions us stands out, as a message that does.
    let style = if *mention {
        fg(theme.warning).add_modifier(Modifier::BOLD)
    } else {
        fg(theme.muted)
    };
    spans.push(Span::styled(text.clone(), style));
    if !time.is_empty() {
        spans.push(Span::styled(
            format!(" · {time}"),
            fg(theme.muted).add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

/// A system row's icon's colour: the role zirc-chat names for it ([`Icon::tone`]).
fn icon_style(app: &App, icon: Icon) -> Style {
    let theme = app.theme;
    fg(theme.role(icon.tone()).unwrap_or(theme.muted))
}

/// A nick's colour: ours in zirc's signal, everyone else's as text.
fn nick_style(app: &App, nick: &str) -> Style {
    if nick.eq_ignore_ascii_case(app.nick()) {
        fg(app.theme.signal).add_modifier(Modifier::BOLD)
    } else {
        fg(app.theme.text)
    }
}

/// The member list's width: room for every name, and for saying who's away, within limits and
/// no more than a quarter of the terminal's `width`.
fn members_width(app: &App, width: u16) -> u16 {
    let widest = app
        .chat
        .people_rows(&app.current().name)
        .iter()
        .map(|row| PRESENCE_MARK + row.label().width())
        .chain(
            app.chat
                .app_rows(&app.current().name)
                .iter()
                .map(|row| app_label(row).width()),
        )
        .max()
        .unwrap_or(0);
    // The border, and a space either side.
    let wanted = u16::try_from(widest + 3).unwrap_or(u16::MAX);
    wanted
        .clamp(MEMBERS_WIDTH.0, MEMBERS_WIDTH.1)
        .min((width / 4).max(MEMBERS_WIDTH.0))
}

/// The columns a presence mark takes before a name, with its space.
const PRESENCE_MARK: usize = 2;

/// Someone's presence as a small mark in its colour: ● here, ◐ away, ○ gone.
fn presence_mark(theme: &Theme, presence: Presence) -> Span<'static> {
    let (mark, colour) = match presence {
        Presence::Here => ("● ", theme.presence_here),
        Presence::Away => ("◐ ", theme.presence_away),
        Presence::Gone => ("○ ", theme.muted),
    };
    Span::styled(mark, fg(colour))
}

fn members_block() -> Block<'static> {
    Block::new().borders(Borders::LEFT)
}

fn members(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = app.theme;
    let block = side(members_block(), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width).saturating_sub(1);
    // One row per person, their machines' agents counted on it: who is changing what
    // is on a session's card, not here. One People list, in zirc-chat's order (operators,
    // then by name), as every app lists them (the user's choice, 10 Nov
    // 2026, over grouping them under headings).
    let mut lines = Vec::new();
    let people = app.chat.people_rows(&app.current().name);
    if !people.is_empty() {
        lines.push(Line::styled(
            format!(" {}", section_label(zirc_core::chat::PEOPLE_SECTION)),
            fg(theme.muted).add_modifier(Modifier::BOLD),
        ));
    }
    // Each by zirc-chat's presence mark; those not here, which includes people with no app
    // connected, are quieter.
    lines.extend(people.iter().map(|row| {
        let presence = row.standing().presence();
        let label = fit(&row.label(), width.saturating_sub(PRESENCE_MARK));
        let style = match presence {
            Presence::Here => nick_style(app, &row.person.nick),
            Presence::Away | Presence::Gone => fg(theme.muted),
        };
        Line::from(vec![
            Span::raw(" "),
            presence_mark(theme, presence),
            Span::styled(label, style),
        ])
    }));
    // zirc's apps, CI and the like, on their own under the people: by their label, with
    // no presence, as they're always there.
    let apps = app.chat.app_rows(&app.current().name);
    if !apps.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!(" {}", section_label(APPS_SECTION)),
            fg(theme.muted).add_modifier(Modifier::BOLD),
        ));
        lines.extend(apps.iter().map(|row| {
            let tag = format!(" {APP_TAG}");
            let label = fit(&row.label, width.saturating_sub(tag.width()));
            Line::from(vec![
                Span::styled(format!(" {label}"), fg(theme.text)),
                Span::styled(tag, fg(theme.muted)),
            ])
        }));
    }
    let lines: Vec<Line<'static>> = lines.into_iter().map(|line| stale(app, line)).collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One of zirc's apps in the member list: its name and zirc-chat's tag, `CI APP`, as its
/// rows head what it says, and no presence mark.
fn app_label(row: &zirc_core::chat::AppRow) -> String {
    format!("{} {APP_TAG}", row.label)
}

fn status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    // Leaving a channel waits on y, asked on this line over the input in zirc-chat's words.
    if let Some(prompt) = app.leaving() {
        let theme = app.theme;
        // The keys in full where they fit, else the one that leaves.
        let room = usize::from(area.width).saturating_sub(prompt.width() + 3);
        let keys = [LEAVE_KEYS, LEAVE_KEY]
            .into_iter()
            .find(|keys| keys.width() <= room)
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {prompt}"), fg(theme.text)),
                Span::styled(
                    format!("  {keys}"),
                    fg(theme.muted).add_modifier(Modifier::DIM),
                ),
            ])),
            area,
        );
        return;
    }
    frame.render_widget(Paragraph::new(status_left(app)), area);
    // What needs the person, at the right: the connection, only when it isn't up, and
    // what mentions them.
    frame.render_widget(
        Paragraph::new(status_right(app)).alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

/// What the keys do while leaving a channel waits on the person.
const LEAVE_KEYS: &str = "y leaves · any other key keeps it";
/// [`LEAVE_KEYS`] where they don't fit.
const LEAVE_KEY: &str = "y leaves";

/// The status line's left: where the person is, `Acme · #web`, then what's going on
/// here, its keys dim.
fn status_left(app: &App) -> Line<'static> {
    let theme = app.theme;
    let hint = fg(theme.muted).add_modifier(Modifier::DIM);
    let mut spans = Vec::new();
    // The organisation as signed in to it, else as the server names it, else its server.
    let org = app
        .org
        .clone()
        .or_else(|| Some(app.chat.org_header().name).filter(|name| !name.is_empty()))
        .unwrap_or_else(|| app.server().to_owned());
    spans.extend([
        Span::styled(format!(" {org} · "), fg(theme.muted)),
        Span::styled(
            app.chat.header_title(&app.current().name),
            fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    if let Some(why) = app.chat.away(app.nick()) {
        spans.push(Span::styled(
            format!("  · {}", away_label(why)),
            fg(theme.warning),
        ));
        spans.push(Span::styled(
            format!(", {}", zirc_core::chat::labels::tui::AWAY_BACK),
            hint,
        ));
    }
    // A card's button waits on y or n.
    if let Some(pressing) = app.pressing() {
        spans.push(Span::styled(
            format!("  · {} y/n", pressing.ask),
            fg(theme.warning).add_modifier(Modifier::BOLD),
        ));
    }
    if app.people_only() {
        spans.push(Span::styled(
            format!("  · {}", zirc_core::chat::labels::tui::PEOPLE_ONLY),
            hint,
        ));
    }
    if let Some(showing) = app.showing() {
        spans.push(Span::styled(
            format!("  · {showing}, /show to change"),
            hint,
        ));
    }
    if let Some(logged) = app.chat.status().logged {
        spans.push(Span::styled(
            format!("  · {}", logged.label()),
            fg(theme.warning),
        ));
    }
    if let Some(notice) = app.chat.status().recent((app.chat.clock)()) {
        spans.push(Span::styled(
            format!("  · {}", notice.line()),
            fg(theme.muted),
        ));
    }
    if let Some(after) = app.editing_after() {
        spans.push(Span::styled(
            format!("  · {}{after}", zirc_core::chat::EDITING),
            fg(theme.signal),
        ));
        spans.push(Span::styled(
            format!(", {}", zirc_core::chat::labels::tui::ESC_CANCELS),
            hint,
        ));
    }
    if let Some(bar) = app.reply_bar() {
        spans.push(Span::styled(
            format!("  · {}", bar.line()),
            fg(theme.signal),
        ));
        spans.push(Span::styled(
            format!(", {}", zirc_core::chat::labels::tui::ESC_CANCELS),
            hint,
        ));
    }
    if let Some(copied) = app.flash() {
        spans.push(Span::styled(format!("  · {copied}"), fg(theme.ok)));
    } else if app.picked().is_some() && app.input.text().is_empty() {
        spans.push(Span::styled(
            "  · ↑/↓ pick, y copies, l its link, e edits yours, s unseals, i details, Esc lets go",
            hint,
        ));
    }
    if app.scrolled_back() {
        spans.push(Span::styled(
            format!("  · {}", zirc_core::chat::labels::tui::SCROLLED_BACK),
            hint,
        ));
    }
    Line::from(spans)
}

/// The status line's right: the connection when it isn't up, in its colour; what mentions
/// the person, on the badge colour, and in the other organisations.
fn status_right(app: &App) -> Line<'static> {
    let theme = app.theme;
    let hint = fg(theme.muted).add_modifier(Modifier::DIM);
    let mut spans = Vec::new();
    // The connection in a word, while it isn't up: the banner over the scrollback says
    // the rest.
    if let Some(word) = app.connection.short() {
        spans.push(Span::styled(format!("{word} "), fg(theme.warning)));
    }
    // What the organisation's badge counts, as every app's does ([`Chat::badge_total`]):
    // what mentions us in a channel and every message in a direct one, the muted left out.
    let mentions = app.chat.badge_total();
    if mentions > 0 {
        spans.push(Span::styled(
            format!(" {} ", mentions_label(mentions)),
            fg(theme.on_badge)
                .bg(colour(theme.badge))
                .add_modifier(Modifier::BOLD),
        ));
        // words-guard: a key's name, which jumps to the next mention
        spans.push(Span::styled(" Alt-A ", hint));
    }
    if let Some(elsewhere) = elsewhere(app) {
        spans.push(Span::styled(format!("{elsewhere} "), fg(theme.warning)));
    }
    Line::from(spans)
}

/// What's unread in the other organisations, on a client signed in to several: `  · globex
/// 3 (1 mention), Alt-O`.
fn elsewhere(app: &App) -> Option<String> {
    let others: Vec<String> = app
        .orgs
        .iter()
        .filter(|(org, unread)| Some(org) != app.org.as_ref() && unread.unread > 0)
        .map(|(org, unread)| match unread.mentions {
            0 => format!("{org} {}", unread.unread),
            many => format!("{org} {} ({})", unread.unread, mentions_label(many)),
        })
        .collect();
    (!others.is_empty()).then(|| format!("  · {}, Alt-O", others.join(", ")))
}

/// How many rows the input takes: one for each line typed, up to as many as every app's field
/// grows to ([`zirc_core::compose::FIELD_LINES`]) and two fifths of the screen.
fn input_height(app: &App, area: Rect) -> u16 {
    let lines = app.input.text().split('\n').count();
    let most = u16::try_from(zirc_core::compose::FIELD_LINES).unwrap_or(u16::MAX);
    u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .min(most)
        .min((area.height * 2 / 5).max(1))
}

fn input(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = app.chat.title(&app.current().name);
    let prompt = format!("{title} › ");
    let text = app.input.text();
    let before: String = text.chars().take(app.input.cursor()).collect();
    // The line the cursor is on, and what's before it on that line.
    let row = before.matches('\n').count();
    let cursor = before.rsplit('\n').next().unwrap_or_default();
    let room = usize::from(area.width).saturating_sub(prompt.width() + 1);
    // Scroll the cursor's line sideways to keep the cursor in view.
    let mut shown_from = 0;
    while cursor[shown_from..].width() > room {
        shown_from += cursor[shown_from..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
    }
    // Lines after the first line up under its text, and the cursor's stays in view.
    let height = usize::from(area.height).max(1);
    let first = (row + 1).saturating_sub(height);
    let lines: Vec<Line<'_>> = text
        .split('\n')
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(i, line)| {
            let lead = if i == 0 {
                Span::styled(prompt.clone(), fg(app.theme.channel))
            } else {
                Span::raw(" ".repeat(prompt.width()))
            };
            let shown = if i == row { &line[shown_from..] } else { line };
            Line::from(vec![lead, Span::styled(shown, fg(app.theme.text))])
        })
        .collect();
    // Where nobody reads what's written, as zirc's own direct messages, the line says so.
    let lines = match app.chat.read_only_note(&app.current().name) {
        Some(note) if text.is_empty() => vec![Line::from(vec![
            Span::styled(prompt.clone(), fg(app.theme.channel)),
            Span::styled(note.text, fg(app.theme.muted)),
        ])],
        _ => lines,
    };
    frame.render_widget(Paragraph::new(lines), area);
    let x = prompt.width() + cursor[shown_from..].width();
    frame.set_cursor_position(Position {
        x: area.x
            + u16::try_from(x)
                .unwrap_or(u16::MAX)
                .min(area.width.saturating_sub(1)),
        y: area.y + u16::try_from(row - first).unwrap_or(0),
    });
}

#[cfg(test)]
mod scroll_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Link};
    use crate::rows::Zone;
    use chrono::{DateTime, FixedOffset, TimeZone as _, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use zirc_core::chat::{
        BADGE_MOST, BROWSE_CHANNELS, MACHINES, NO_OTHER_CHANNELS, PEOPLE_SECTION, SERVER_TITLE,
        Shelf, badge_label,
    };

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
    }

    fn app() -> App {
        let mut app = App::new("me", "irc.zirc.test", Vec::new());
        app.chat.clock = noon;
        app.zone = Zone::Fixed(FixedOffset::east_opt(0).unwrap());
        app.connected(Link {
            nick: "me".into(),
            metadata: false,
            chathistory: true,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
        });
        let commit = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-12",
            zirc_proto::Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: "abc".into(),
                subject: "Fix login".into(),
                repo: None,
            },
        ))
        .unwrap();
        for line in [
            ":me!u@h JOIN #api".to_owned(),
            ":s 332 me #api :the API".to_owned(),
            ":s 353 me = #api :me @alice bob mattias-mbp me-laptop".to_owned(),
            ":s 366 me #api :End".to_owned(),
            // Our laptop, as its host says.
            ":s 354 me 42 machine/me me-laptop H me-laptop :me".to_owned(),
            ":me!u@h JOIN #web".to_owned(),
            // Joining showed #api; look at the server's to count what's unread.
            "SELECT 0".to_owned(),
            "@time=2026-10-30T11:00:00.000Z :alice!u@h PRIVMSG #api :morning".to_owned(),
            "@time=2026-10-30T11:01:00.000Z :alice!u@h PRIVMSG #api :me: can you look?".to_owned(),
            format!(
                "@time=2026-10-30T11:02:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{commit}"
            ),
            "@time=2026-10-30T11:03:00.000Z :bob!u@h PRIVMSG #web :deploying".to_owned(),
        ] {
            if line == "SELECT 0" {
                let _ = app.select(0);
            } else {
                app.handle(line.parse().unwrap());
            }
        }
        app
    }

    fn draw(app: &App, width: u16, height: u16) -> (Vec<String>, Terminal<TestBackend>) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        (rows, terminal)
    }

    #[test]
    fn i_on_a_picked_message_shows_its_details_and_data_until_esc() {
        let mut app = app();
        let _ = app.select(1);
        let mut post = zirc_proto::Message::new(
            "cc-7",
            zirc_proto::Event::Text {
                body: "taking: the tests".into(),
            },
        );
        post.task = Some("t-abc123".into());
        let body = zirc_proto::encode(&post).unwrap();
        app.handle(
            format!(
                "@time=2026-10-30T11:04:00.000Z;msgid=m9 :mattias-mbp!u@machine/mattias PRIVMSG #api :{body}"
            )
            .parse()
            .unwrap(),
        );
        let screen = Rect::new(0, 0, 120, 40);
        app.view = scrollback_size(&app, screen);
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        assert_eq!(app.picked(), Some("m9"));
        let (rows, _) = draw(&app, screen.width, screen.height);
        row_with(&rows, "i details");
        app.key(KeyEvent::from(KeyCode::Char('i')));
        let (rows, _) = draw(&app, screen.width, screen.height);
        row_with(&rows, zirc_core::chat::MESSAGE_DETAILS);
        assert!(row_with(&rows, "Session ").contains("cc-7"));
        assert!(row_with(&rows, "Task ").contains("t-abc123"));
        row_with(&rows, zirc_core::chat::DATA);
        row_with(&rows, "\"task\": \"t-abc123\"");
        // ↓ scrolls it rather than picking another message.
        app.key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.picked(), Some("m9"));
        app.key(KeyEvent::from(KeyCode::Esc));
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(
            !rows
                .iter()
                .any(|row| row.contains(zirc_core::chat::MESSAGE_DETAILS))
        );
        assert_eq!(app.picked(), Some("m9"), "Esc shuts the details first");
    }

    fn row_with<'a>(rows: &'a [String], text: &str) -> &'a str {
        rows.iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?} in\n{}", rows.join("\n")))
    }

    /// Where the sidebar's heading `label` is drawn, as a column and row to click: its own
    /// row, not the switch's half of the same name above it.
    fn heading_at(rows: &[String], label: &str) -> (u16, u16) {
        let y = rows
            .iter()
            .position(|row| row.split('│').next().unwrap().trim_end() == format!(" {label}"))
            .unwrap_or_else(|| panic!("no heading {label:?} in\n{}", rows.join("\n")));
        (1, u16::try_from(y).unwrap())
    }

    /// Where `text` is drawn on the screen, as a column and row to click.
    fn place_of(rows: &[String], text: &str) -> (u16, u16) {
        let (y, row) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains(text))
            .unwrap_or_else(|| panic!("no {text:?} in\n{}", rows.join("\n")));
        let x = row[..row.find(text).unwrap()].chars().count();
        (u16::try_from(x).unwrap(), u16::try_from(y).unwrap())
    }

    #[test]
    fn every_icon_is_coloured_in_a_role_the_theme_has() {
        let theme = zirc_theme::Theme::dark();
        let icons = [
            Icon::Push,
            Icon::Commit,
            Icon::Working,
            Icon::Intent,
            Icon::Taking,
            Icon::Landed,
            Icon::Failed,
            Icon::Conflict,
            Icon::Overlap,
            Icon::Join,
            Icon::Leave,
            Icon::Post,
        ];
        for icon in icons {
            assert!(
                theme.role(icon.tone()).is_some(),
                "{icon:?} is coloured in {}, which zirc-theme doesn't have",
                icon.tone()
            );
        }
        assert_eq!(theme.role(Icon::Landed.tone()), Some(theme.ok));
        assert_eq!(theme.role(Icon::Failed.tone()), Some(theme.error));
        assert_eq!(theme.role(Icon::Conflict.tone()), Some(theme.warning));
        assert_eq!(theme.role(Icon::Overlap.tone()), Some(theme.warning));
        assert_eq!(theme.role(Icon::Intent.tone()), Some(theme.muted));
    }

    #[test]
    fn searches_here_or_everywhere_as_zirc_chat_lays_it_out_and_keeps_what_was_found() {
        use zirc_core::chat::{RECENT_TITLE, SEARCH, labels};
        let mut app = app();
        app.handle(
            "@msgid=m7;time=2026-10-30T11:04:00.000Z :bob!u@h PRIVMSG #api :look at the lookup tables"
                .parse()
                .unwrap(),
        );
        let api = app.chat.position("#api").unwrap();
        let _ = app.select(api);
        let alt_f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT);
        let _ = app.key(alt_f);
        let (rows, _) = draw(&app, 100, 30);
        row_with(
            &rows,
            &labels::with_shortcut(SEARCH, labels::tui::SEARCH_KEY),
        );
        for c in "look".chars() {
            let _ = app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let (rows, _) = draw(&app, 100, 30);
        let screen = app.search_screen().unwrap();
        row_with(&rows, screen.header.as_deref().unwrap());
        row_with(&rows, "me: can you look?");
        // Tab: everywhere, as the control beside the box says.
        let _ = app.key(KeyEvent::from(KeyCode::Tab));
        assert!(
            app.chat
                .search_scope(&app.searching().unwrap().finding.query())
                .is_none()
        );
        let _ = app.key(KeyEvent::from(KeyCode::Tab));
        // Enter opens the hit picked and picks its message; search stays open.
        let first = app.search_screen().unwrap().listed_hits()[0].clone();
        assert_eq!(first.id.as_deref(), Some("m7"), "newest first: {first:?}");
        let _ = app.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.current().name, "#api");
        assert_eq!(app.picked(), Some("m7"));
        assert!(app.searching().is_some());
        let _ = app.key(KeyEvent::from(KeyCode::Esc));
        assert!(app.searching().is_none());
        // What was searched for is kept, and listed while nothing's typed.
        let _ = app.key(alt_f);
        let (rows, _) = draw(&app, 100, 30);
        row_with(&rows, &Type::get().label(RECENT_TITLE));
        assert_eq!(
            app.prefs().recent_searches.values().next().unwrap().len(),
            1
        );
    }

    #[test]
    fn opens_a_long_message_with_enter_as_its_row_says() {
        let mut app = app();
        // Twelve lines, as a multiline batch brings them: more than the core shows of one.
        app.handle(
            "@msgid=m9;time=2026-10-30T11:05:00.000Z :bob!u@h BATCH +b1 draft/multiline #api"
                .parse()
                .unwrap(),
        );
        for n in 1..=12 {
            app.handle(
                format!("@batch=b1 :bob!u@h PRIVMSG #api :step {n}")
                    .parse()
                    .unwrap(),
            );
        }
        app.handle(":bob!u@h BATCH -b1".parse().unwrap());
        let api = app.chat.position("#api").unwrap();
        let _ = app.select(api);
        let _ = app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        assert_eq!(app.picked(), Some("m9"));
        let (rows, _) = draw(&app, 100, 40);
        row_with(&rows, "more lines · Enter shows them");
        assert!(!rows.iter().any(|row| row.contains("step 12")));
        let _ = app.key(KeyEvent::from(KeyCode::Enter));
        let (rows, _) = draw(&app, 100, 40);
        row_with(&rows, "step 12");
        row_with(&rows, "Enter shows less");
    }

    #[test]
    fn jumps_where_whats_typed_names_as_zirc_chat_lays_it_out() {
        use zirc_core::chat::{JUMP_TO, labels, nothing_called};
        let mut app = app();
        let _ = app.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        let (rows, _) = draw(&app, 100, 24);
        row_with(
            &rows,
            &labels::with_shortcut(JUMP_TO, labels::tui::JUMP_KEY),
        );
        for c in "we".chars() {
            let _ = app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let (rows, _) = draw(&app, 100, 24);
        // What the core finds, then the channel to make for what's typed.
        row_with(&rows, "Create #we");
        let _ = app.key(KeyEvent::from(KeyCode::Enter));
        assert!(app.jumping().is_none());
        assert_eq!(app.current().name, "#web");

        let _ = app.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        for c in "zz zz".chars() {
            let _ = app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let (rows, _) = draw(&app, 100, 24);
        row_with(&rows, &nothing_called("zz zz"));
        let _ = app.key(KeyEvent::from(KeyCode::Esc));
        assert!(app.jumping().is_none());
    }

    #[test]
    fn draws_links_as_their_labels_clickable_or_as_written() {
        let mut app = app();
        app.handle(
            "@time=2026-10-30T11:05:00.000Z :bob!u@h PRIVMSG #api :see https://github.com/zircapp/zirc/pull/12?tab=files please"
                .parse()
                .unwrap(),
        );
        let _ = app.select(1);
        // Where the terminal takes OSC 8: the label, linking to the whole address.
        app.links = crate::hyperlink::Links::Labelled;
        let (rows, _) = draw(&app, 160, 16);
        let row = row_with(&rows, "please");
        let url = "https://github.com/zircapp/zirc/pull/12?tab=files";
        assert!(
            row.contains(&format!(
                "see \x1b]8;;{url}\x1b\\github.com/zircapp/zirc/pull/12\x1b]8;;\x1b\\ please"
            )),
            "{row:?}"
        );
        // Elsewhere, the address as written, and no escapes.
        app.links = crate::hyperlink::Links::Plain;
        let (rows, _) = draw(&app, 160, 16);
        let row = row_with(&rows, "please");
        assert!(row.contains(&format!("see {url} please")), "{row:?}");
        assert!(!row.contains('\x1b'));
    }

    #[test]
    fn lists_active_threads_and_leaves_quiet_ones_to_the_channel() {
        let mut app = app();
        for line in [
            ":me!u@h JOIN #web/fresh",
            ":me!u@h JOIN #web/old-a",
            ":me!u@h JOIN #web/old-b",
            "@time=2026-10-30T11:00:00.000Z :me!u@h PRIVMSG #web/fresh :today",
            "@time=2026-10-20T11:00:00.000Z :me!u@h PRIVMSG #web/old-a :weeks ago",
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 100, 20);
        // Threads by title with a branch mark, as every app lists them: only the active one,
        // the quiet ones (nothing new for a day) left to the channel (the core's `quiet`).
        row_with(&rows, "└ fresh");
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("old a") || row.contains("old b")),
            "{rows:#?}"
        );
    }

    #[test]
    fn shows_people_by_the_names_their_profiles_give() {
        let mut app = app();
        let _ = app.select(1);
        for line in [
            ":s METADATA alice display-name * :Alice Liddell",
            "@time=2026-10-30T11:30:00.000Z :alice!u@h PRIVMSG me :psst",
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 120, 20);
        // Heading what she said, in the member list, and in the direct messages.
        assert!(row_with(&rows, "Alice Liddell  11:0").contains("Alice Liddell  11:00"));
        assert!(row_with(&rows, "Alice Liddell").ends_with("│ ● Alice Liddell"));
        row_with(&rows, " Alice Liddell ");
    }

    #[test]
    fn names_agent_sessions_and_says_who_a_post_is_for() {
        let mut app = app();
        let _ = app.select(1);
        let post = |session: &str, name: Option<&str>, body: &str| {
            let mut message =
                zirc_proto::Message::new(session, zirc_proto::Event::Text { body: body.into() });
            message.name = name.map(str::to_owned);
            zirc_proto::encode(&message).unwrap()
        };
        for line in [
            format!(
                "@time=2026-10-30T11:20:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                post("cc-598aa1", Some("zirc-chat"), "on it")
            ),
            "@time=2026-10-30T11:20:30.000Z :alice!u@h PRIVMSG #api :nice".to_owned(),
            format!(
                "@time=2026-10-30T11:21:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                post("cc-ad4f7c", None, "cc-598aa1: thanks, landed")
            ),
            "@time=2026-10-30T11:22:00.000Z :alice!u@h PRIVMSG #api :ask cc-598aa1 about it"
                .to_owned(),
            // Its later lines needn't name it again.
            format!(
                "@time=2026-10-30T11:23:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                post("cc-598aa1", None, "pushed")
            ),
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, terminal) = draw(&app, 120, 24);
        // A session heads all it says by the name it goes by, never its id.
        let id = rows.iter().find(|row| row.contains("cc-598aa1"));
        assert_eq!(id, None, "{}", rows.join("\n"));
        // A post to a session says so first, by the session's name.
        row_with(&rows, "→ zirc-chat thanks, landed");
        // In what people say, a session goes by its name, drawn as a mention.
        let (x, y) = place_of(&rows, "ask zirc-chat about it");
        let at = u16::try_from("ask ".len()).unwrap();
        let cell = &terminal.backend().buffer()[(x + at, y)];
        assert_eq!(cell.symbol(), "z");
        assert_eq!(cell.fg, colour(app.theme.mention_chip_text));
        assert_eq!(cell.bg, colour(app.theme.mention_chip_ground));
        assert_ne!(
            terminal.backend().buffer()[(x, y)].fg,
            colour(app.theme.signal)
        );
    }

    #[test]
    fn links_a_pushs_commit_and_branch_to_their_pages() {
        let mut app = App::new("me", "irc.zirc.test", Vec::new());
        app.chat.clock = noon;
        app.zone = Zone::Fixed(FixedOffset::east_opt(0).unwrap());
        app.links = crate::hyperlink::Links::Labelled;
        app.chat
            .set_threshold("#api", zirc_core::chat::COMMITS_AND_PUSHES_SHOW_AT);
        app.handle(":me!u@h JOIN #api".parse().unwrap());
        let _ = app.select(app.chat.position("#api").unwrap());
        let push = zirc_proto::Message::new(
            "cc-598aa1",
            zirc_proto::Event::Push {
                branch: "fix-login".into(),
                head: "abc1234def5678".into(),
                forced: false,
                subjects: vec!["Fix the login form".into()],
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: Some("https://github.com/zircapp/zirc".into()),
                on: None,
            },
        );
        app.handle(
            format!(
                "@time=2026-10-30T11:45:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                zirc_proto::encode(&push).unwrap()
            )
            .parse()
            .unwrap(),
        );
        // In the channel it's one quiet line, zirc-chat's brief, which links nothing; whole in
        // the agent's own conversation.
        let _ = app.chat.open("mattias-mbp/cc-598aa1");
        assert_eq!(app.current().name, "mattias-mbp/cc-598aa1");
        let (rows, _) = draw(&app, 160, 24);
        let row = row_with(&rows, "pushed");
        // The commit and the branch, each a link to its page on the forge.
        let repo = "https://github.com/zircapp/zirc";
        assert!(
            row.contains(&format!(
                "\x1b]8;;{repo}/commit/abc1234def5678\x1b\\abc1234\x1b]8;;\x1b\\"
            )),
            "{row:?}"
        );
        assert!(
            row.contains(&format!(
                "\x1b]8;;{repo}/tree/fix-login\x1b\\fix-login\x1b]8;;\x1b\\"
            )),
            "{row:?}"
        );
    }

    #[test]
    fn shows_agents_talk_as_rows_and_only_people_on_asking() {
        let mut app = app();
        let _ = app.select(1);
        let post = |session: &str, name: &str, body: &str, minute: u32| {
            let mut message =
                zirc_proto::Message::new(session, zirc_proto::Event::Text { body: body.into() });
            message.name = Some(name.into());
            format!(
                "@time=2026-10-30T11:{minute:02}:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                zirc_proto::encode(&message).unwrap()
            )
        };
        for line in [
            post(
                "cc-598aa1",
                "zirc-chat",
                "can you take the schema change",
                40,
            ),
            post("cc-ad4f7c", "controller", "on it", 41),
            post("cc-598aa1", "zirc-chat", "thanks", 42),
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 140, 24);
        // Agents' posts are rows of their own, never folded into a thread.
        row_with(&rows, "on it");
        assert!(!rows.iter().any(|row| row.contains("3 messages")));
        // Only people: agents' talk goes, and the status line says how to have it back.
        for c in "/people".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter));
        let (rows, _) = draw(&app, 140, 24);
        assert!(!rows.iter().any(|row| row.contains("on it")));
        row_with(&rows, "people only, /people shows all");
        row_with(&rows, "morning");
    }

    #[test]
    fn shows_each_of_an_agents_doings_never_folded() {
        let mut app = app();
        app.chat
            .set_threshold("#api", zirc_core::chat::COMMITS_AND_PUSHES_SHOW_AT);
        let _ = app.select(1);
        // A second commit after the one already in #api: both show, nothing folds.
        let commit = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-34",
            zirc_proto::Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: "def".into(),
                subject: "Tidy the form".into(),
                repo: None,
            },
        ))
        .unwrap();
        app.handle(
            format!("@time=2026-10-30T11:03:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{commit}")
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, 140, 24);
        row_with(&rows, "Fix login");
        row_with(&rows, "Tidy the form");
        assert!(!rows.iter().any(|row| row.contains("updates")), "{rows:#?}");
    }

    #[test]
    fn draws_a_cards_buttons_and_presses_one_once_confirmed() {
        let mut app = app();
        let _ = app.select(1);
        let card = |note: &str| {
            let message = zirc_proto::Message::new(
                "ci",
                zirc_proto::Event::Custom {
                    kind: "x-acme/deploy".into(),
                    data: format!(
                        r#"{{"card":{{"id":"d/42","title":"web · deploy 42","state":"failed","fields":[{{"label":"failed in","value":"smoke"}}],"actions":[{{"id":"retry","label":"Redeploy"}},{{"id":"cancel","label":"Abort","who":"admins"}}],"note":"{note}"}}}}"#
                    )
                    .parse()
                    .unwrap(),
                    text: "failed".into(),
                },
            );
            format!(
                "@time=2026-10-30T11:50:00.000Z :zirc-alerts!u@h PRIVMSG #api :{}",
                // A card goes as a multiline message, longer than a line.
                zirc_proto::encode_up_to(&message, 2_000).unwrap()
            )
        };
        app.handle(card("1st").parse().unwrap());
        let (rows, _) = draw(&app, 140, 24);
        row_with(&rows, "web · deploy 42");
        row_with(&rows, "failed in: smoke");
        // The buttons we may press: Abort is for admins.
        // Tonal words, apart from the card's text.
        let (x, y) = place_of(&rows, " Redeploy ");
        let buffer = draw(&app, 140, 24).1;
        assert_eq!(
            buffer.backend().buffer()[(x, y)].bg,
            colour(app.theme.tonal_ground)
        );
        assert!(!rows.iter().any(|row| row.contains("Abort")));

        // /press presses through zirc.dev at once: the card doesn't ask first, so no app
        // does (the user's choice, 20 Nov 2026).
        for c in "/press retry".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let Action::Press(press) = app.key(KeyEvent::from(KeyCode::Enter)) else {
            panic!("Enter presses");
        };
        assert_eq!(
            (
                press.sender.as_str(),
                press.card.as_str(),
                press.action.as_str()
            ),
            ("zirc-alerts", "d/42", "retry")
        );
        // What came of it shows beside its buttons, as every app shows it, until the card
        // changes.
        app.press_answered(&press, Ok(Some("Retrying smoke tests".into())));
        let (rows, _) = draw(&app, 140, 24);
        assert!(row_with(&rows, "Retrying smoke tests").contains(" Redeploy "));
        app.handle(card("retried").parse().unwrap());
        let (rows, _) = draw(&app, 140, 24);
        assert!(!rows.iter().any(|row| row.contains("Retrying smoke tests")));
        row_with(&rows, "retried");
    }

    #[test]
    fn marks_an_event_that_mentions_us_as_a_message_that_does() {
        let mut app = app();
        let _ = app.select(2);
        let post = zirc_proto::Message::new(
            "cc-598aa1",
            zirc_proto::Event::Text {
                body: "me: can you review?".into(),
            },
        );
        app.handle(
            format!(
                "@time=2026-10-30T11:50:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #web :{}",
                zirc_proto::encode(&post).unwrap()
            )
            .parse()
            .unwrap(),
        );
        let (rows, terminal) = draw(&app, 140, 24);
        // Its body on the mention ground, the edge in front.
        let (x, y) = place_of(&rows, "▎ me: can you review?");
        assert_eq!(
            terminal.backend().buffer()[(x + 2, y)].bg,
            colour(app.theme.mention_ground)
        );
    }

    #[test]
    fn heads_an_agents_rows_with_its_tools_mark_in_its_own_colour() {
        let mut app = app();
        let _ = app.select(1);
        let mut post = zirc_proto::Message::new(
            "cc-598aa1",
            zirc_proto::Event::Text {
                body: "on it".into(),
            },
        );
        post.name = Some("zirc-chat".into());
        post.tool = Some(zirc_proto::CLAUDE_CODE.into());
        for line in [
            format!(
                "@time=2026-10-30T11:20:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                zirc_proto::encode(&post).unwrap()
            ),
            "@time=2026-10-30T11:30:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :plain words".to_owned(),
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, terminal) = draw(&app, 120, 24);
        let buffer = terminal.backend().buffer();
        // Its post: Claude Code's mark, its name, whose agent it is; no AGENT tag.
        let (x, y) = place_of(&rows, "✳ zirc-chat mattias's agent  11:20");
        let mark = buffer[(x, y)].fg;
        assert_ne!(mark, colour(app.theme.muted));
        assert!(!rows.iter().any(|row| row.contains(AGENT_TAG)));
        // The machine itself, with no session, is headed with its own name, not as an agent.
        let _ = place_of(&rows, "mbp  11:30");
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("mattias's agent  11:30"))
        );
    }

    #[test]
    fn shows_a_landing_as_one_card_with_its_build_linked() {
        let mut app = app();
        app.links = crate::hyperlink::Links::Labelled;
        let _ = app.select(1);
        let post = |kind: &str, text: &str| {
            let message = zirc_proto::Message::new(
                "ci",
                zirc_proto::Event::Custom {
                    kind: kind.into(),
                    data: r#"{"landing": "monorepo/771", "build": "771", "branch": "fix-login"}"#
                        .parse()
                        .unwrap(),
                    text: text.into(),
                },
            );
            format!(
                "@time=2026-10-30T11:40:00.000Z :zirc-alerts!u@h PRIVMSG #api :{}",
                zirc_proto::encode(&message).unwrap()
            )
        };
        for line in [
            post(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: landed fix-login as abc1234 (build 771) https://ci/builds/771",
            ),
            post(
                zirc_proto::ci::DEPLOY_KIND,
                "cc-1: production runs the web apps as abc1234 has them",
            ),
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 140, 24);
        // One card, how it went in place, its outcome in front, as zirc-chat words its line;
        // its build a link to its page; the session it's about dim after it.
        let row = row_with(&rows, "production updated");
        assert!(
            row.contains(
                "✓ fix-login · \x1b]8;;https://ci/builds/771\x1b\\build 771\x1b]8;;\x1b\\ · landed · ✓ production updated · cc-1"
            ),
            "{row:?}"
        );
        assert_eq!(
            rows.iter().filter(|row| row.contains("fix-login")).count(),
            1
        );
    }

    #[test]
    fn quotes_an_agent_by_its_name_never_its_machines_nick() {
        let mut app = app();
        let _ = app.select(1);
        let mut post = zirc_proto::Message::new(
            "cc-598aa1",
            zirc_proto::Event::Text {
                body: "tests pass".into(),
            },
        );
        post.name = Some("zirc-chat".into());
        for line in [
            format!(
                "@msgid=a1;time=2026-10-30T11:40:00.000Z :mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                zirc_proto::encode(&post).unwrap()
            ),
            "@msgid=b1;time=2026-10-30T11:41:00.000Z;+draft/reply=a1 :bob!u@h PRIVMSG #api :nice"
                .to_owned(),
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 120, 24);
        row_with(&rows, "↪ zirc-chat: tests pass");
        assert!(!rows.iter().any(|row| row.contains("↪ mattias-mbp")));
    }

    #[test]
    fn lists_zircs_apps_apart_from_the_people() {
        let mut app = app();
        let _ = app.select(1);
        app.handle(":zirc-ci!u@h JOIN #api".parse().unwrap());
        let (rows, terminal) = draw(&app, 120, 20);
        // Not a member like the people: in their own part, by name and zirc-chat's tag,
        // dimmed, with no presence.
        assert!(row_with(&rows, PEOPLE_SECTION).ends_with(&format!("│ {PEOPLE_SECTION}")));
        assert!(row_with(&rows, APPS_SECTION).ends_with(&format!("│ {APPS_SECTION}")));
        assert!(row_with(&rows, "CI APP").ends_with("│ CI APP"));
        let (x, y) = place_of(&rows, "CI APP");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(x, y)].fg, colour(app.theme.text));
        assert_eq!(buffer[(x + 3, y)].fg, colour(app.theme.muted));
        assert!(!rows.iter().any(|row| row.ends_with("│ zirc-ci")));
    }

    #[test]
    fn clicks_land_on_what_is_drawn_there() {
        let mut app = app();
        let _ = app.select(1);
        let screen = Rect::new(0, 0, 100, 16);
        let (rows, _) = draw(&app, screen.width, screen.height);
        let at = |text: &str| {
            let (x, y) = place_of(&rows, text);
            target_at(&app, screen, x, y)
        };
        // By where it is among the conversations, the server's first.
        assert_eq!(at("#web"), Some(Target::Conversation(2)));
        // The switch's halves show theirs, by their words, which fit with two; under it a
        // section's heading folds it.
        assert_eq!(
            at(zirc_core::chat::SidebarSection::Direct.label()),
            Some(Target::Tab(zirc_core::chat::SidebarSection::Direct))
        );
        let (x, y) = heading_at(&rows, Shelf::Channels.label());
        assert_eq!(
            target_at(&app, screen, x, y),
            Some(Target::Shelf(zirc_core::chat::Shelf::Channels))
        );
        assert_eq!(at("alice"), Some(Target::Member("alice".into())));
        assert_eq!(at("○ mattias"), Some(Target::Member("mattias".into())));
        assert_eq!(at("morning"), Some(Target::Scrollback));
        // The member count, the header and the input line aren't anything to click.
        assert_eq!(at(PEOPLE_SECTION), None);
        assert_eq!(at("the API"), None);
        assert_eq!(at("#api ›"), None);
        // The last channel in the list, #web (A–Z), under the header naming it.
        let (x, y) = place_of(&rows[1..], "▎#web");
        assert_eq!(
            target_at(&app, screen, x, y + 1),
            Some(Target::Conversation(2))
        );
    }

    #[test]
    fn shows_conversations_with_unread_counts() {
        let mut app = app();
        app.handle(
            ":me!u@h JOIN #a-channel-with-a-very-long-name"
                .parse()
                .unwrap(),
        );
        let _ = app.select(0);
        let (rows, _) = draw(&app, 100, 16);
        // Unread, with a pip; a pill for what mentions us, none for the rest.
        // What was said last and when, where it fits.
        assert_eq!(row_with(&rows, "#api"), "▎#api cc-12: Fix… 58m 1  │");
        assert!(row_with(&rows, "#web").starts_with("▎#web bob: "));
        // Long names are cut to fit, the same way everywhere.
        assert_eq!(row_with(&rows, " #a-"), " #a-channel-with-a-very… │");
        // The Server channel shown: its name heads the screen, though no list holds it.
        assert!(
            rows[0].starts_with(&format!(" {SERVER_TITLE}")),
            "{}",
            rows[0]
        );
        // Where we are on the left; the connection only when it's down, and what mentions
        // us, on the right.
        let status = row_with(&rows, "irc.zirc.test · ");
        assert!(status.starts_with(" irc.zirc.test · Server"), "{status}");
        assert!(!status.contains("connected"), "{status}");
        assert!(
            status.ends_with(&format!(" {}  Alt-A", mentions_label(1))),
            "{status}"
        );
    }

    #[test]
    fn the_status_lines_mentions_are_capped_as_every_badge_is() {
        let mut app = app();
        for at in 0..120 {
            app.handle(format!(":bob!u@h PRIVMSG #web :me: {at}").parse().unwrap());
        }
        let (rows, _) = draw(&app, 100, 16);
        let status = row_with(&rows, "irc.zirc.test · ");
        let said = format!(" {}  Alt-A", mentions_label(app.chat.badge_total()));
        assert!(said.contains(&badge_label(BADGE_MOST + 1)), "{said}");
        assert!(status.ends_with(&said), "{status}");
    }

    #[test]
    fn says_in_the_status_line_while_messages_are_logged() {
        let mut app = app();
        let _ = app.select(1);
        app.handle(
            format!(
                ":irc.zirc.test 372 me :- {}",
                zirc_core::chat::Logged::Everything.notice()
            )
            .parse()
            .unwrap(),
        );
        let (rows, _) = draw(&app, 140, 16);
        assert!(
            row_with(&rows, "irc.zirc.test · ")
                .contains("#api  · all messages are logged for the owners, direct ones too"),
            "{rows:?}"
        );
        app.disconnected("the server closed the connection");
        let (rows, _) = draw(&app, 140, 16);
        assert!(!row_with(&rows, "irc.zirc.test · ").contains("logged"));
    }

    #[test]
    fn says_what_the_connection_is_doing_in_the_status_line_only() {
        let mut app = app();
        let _ = app.select(1);
        app.handle(
            ":irc.zirc.test NOTICE * :Server restarting"
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, 100, 16);
        assert!(row_with(&rows, "irc.zirc.test · ").contains("#api  · Server restarting"));
        app.disconnected("the server closed the connection");
        let (rows, _) = draw(&app, 100, 16);
        // In a word, in the warning colour; the banner says the rest, and never why.
        let status = row_with(&rows, "irc.zirc.test · ");
        assert!(status.ends_with("offline"), "{status}");
        assert!(!rows.iter().any(|row| row.contains("closed the connection")));
        assert!(
            !rows[1..rows.len() - 2]
                .iter()
                .any(|row| row.contains("disconnected"))
        );
    }

    #[test]
    fn names_people_and_their_machines() {
        let mut app = app();
        let _ = app.select(1);
        // A nick that only looks like a machine's, its host saying nothing of an owner.
        app.handle(":dana-mbp!u@h JOIN #api".parse().unwrap());
        let (rows, _) = draw(&app, 100, 16);
        let members: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.rsplit_once('│').map(|(_, member)| member.trim()))
            .skip_while(|member| *member != PEOPLE_SECTION)
            .skip(1)
            .take_while(|member| !member.is_empty())
            .collect();
        // Operators first; one row per person, a machine folded into its owner's, ours into
        // ours and mattias-mbp into mattias's, who's gone. dana-mbp is someone.
        assert_eq!(
            members,
            ["● alice", "● bob", "● dana-mbp", "● me", "○ mattias"]
        );
    }

    #[test]
    fn makes_room_for_whole_names_and_says_who_is_away() {
        let mut app = app();
        let _ = app.select(1);
        for line in [
            ":s 354 me 42 machine/mattias_e2e mattias_e2e-mac2 H mattias_e2e-mac2 :mattias_e2e",
            ":mattias_e2e-mac2!u@machine/mattias_e2e JOIN #api * :mattias_e2e",
            ":s 354 me 42 h alice G alice :alice",
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 120, 16);
        assert!(
            // A machine is listed as its owner, one row for both, gone while only the
            // machine is here.
            rows.iter().any(|row| row.ends_with("│ ○ mattias_e2e")),
            "{}",
            rows.join("\n")
        );
        // Away, by zirc-chat's presence mark in the warning colour, the name quieter.
        assert!(row_with(&rows, "alice").ends_with("│ ◐ alice"));
        let (x, y) = place_of(&rows, "◐ alice");
        let buffer = draw(&app, 120, 16).1;
        assert_eq!(
            buffer.backend().buffer()[(x, y)].fg,
            colour(app.theme.warning)
        );
        assert_eq!(
            buffer.backend().buffer()[(x + 2, y)].fg,
            colour(app.theme.muted)
        );
        // However long the names, the scrollback keeps three quarters of the screen.
        app.handle(
            ":a-very-long-name-for-a-machine!u@h JOIN #api * :Someone With A Very Long Name Indeed"
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, 120, 16);
        let members = row_with(&rows, PEOPLE_SECTION);
        let border = members.chars().rev().position(|c| c == '│').unwrap();
        let x = members.chars().count() - 1 - border;
        assert_eq!(x, 90, "{}", rows.join("\n"));
    }

    #[test]
    fn shows_reactions_and_the_picker() {
        let mut app = app();
        let _ = app.select(1);
        for line in [
            "@msgid=m1;time=2026-10-30T11:05:00.000Z :alice!u@h PRIVMSG #api :shipped it",
            "@+draft/react=🎉;+draft/reply=m1 :bob!u@h TAGMSG #api",
            "@+draft/react=🎉;+draft/reply=m1 :me!u@h TAGMSG #api",
            "@+draft/react=🚀;+draft/reply=m1 :bob!u@h TAGMSG #api",
        ] {
            app.handle(line.parse().unwrap());
        }
        let screen = Rect::new(0, 0, 100, 16);
        app.view = scrollback_size(&app, screen);
        let (rows, terminal) = draw(&app, screen.width, screen.height);
        let shipped = rows
            .iter()
            .position(|row| row.contains("shipped it"))
            .unwrap();
        let chips = &rows[shipped + 1];
        // An emoji takes two cells, the second of them blank.
        assert!(chips.contains(" 🎉  2   🚀  1"), "{}", rows.join("\n"));
        // Ours stands out in the reaction-mine colours.
        let (x, y) = place_of(&rows, "🎉");
        assert_eq!(
            terminal.backend().buffer()[(x, y)].bg,
            colour(app.theme.reaction_mine_ground)
        );
        // A click on one is that reaction; on the message, the message.
        assert_eq!(
            target_at(&app, screen, x, y),
            Some(Target::Reaction {
                id: "m1".into(),
                emoji: "🎉".into()
            })
        );
        let (x, y) = place_of(&rows, "shipped it");
        assert_eq!(
            target_at(&app, screen, x, y),
            Some(Target::Message("m1".into()))
        );

        // The message picked is marked, and the picker opens over the scrollback.
        app.key(KeyEvent::new(
            KeyCode::Up,
            ratatui::crossterm::event::KeyModifiers::SHIFT,
        ));
        assert_eq!(app.picked(), Some("m1"));
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(row_with(&rows, "shipped it").contains("▌"));
        app.key(KeyEvent::new(
            KeyCode::Char('r'),
            ratatui::crossterm::event::KeyModifiers::ALT,
        ));
        for c in "tada".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let (rows, _) = draw(&app, screen.width, screen.height);
        let card = rows.join("\n");
        assert!(card.contains(" react "), "{card}");
        assert!(row_with(&rows, "search").contains("search tada"), "{card}");
        assert!(card.contains("party popper"), "{card}");
        assert!(
            card.contains(&format!(
                "Enter reacts · {}",
                zirc_core::chat::labels::ESC_CLOSES
            )),
            "{card}"
        );
        // The message reacted to stays in view below it.
        assert!(row_with(&rows, "shipped it").contains("▌"), "{card}");
    }

    #[test]
    fn copies_the_message_picked_or_its_link() {
        let mut app = app();
        let _ = app.select(1);
        for line in [
            "@msgid=m8;time=2026-10-30T11:04:00.000Z :bob!u@h PRIVMSG #api :see https://zirc.dev/m/x.",
            "@msgid=m9;time=2026-10-30T11:05:00.000Z :bob!u@h PRIVMSG #api :no link here",
        ] {
            app.handle(line.parse().unwrap());
        }
        let screen = Rect::new(0, 0, 120, 20);
        app.view = scrollback_size(&app, screen);
        let key = |app: &mut App, code| app.key(KeyEvent::from(code));

        // Nothing picked, y is typed.
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(app.input.text(), "y");
        key(&mut app, KeyCode::Backspace);

        // Shift-↑ picks the newest, and the status line says what's next.
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        assert_eq!(app.picked(), Some("m9"));
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(
            row_with(&rows, "irc.zirc.test · ").contains("↑/↓ pick, y copies, l its link"),
            "{rows:?}"
        );
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.take_clipboard(), None);
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(row_with(&rows, "irc.zirc.test · ").contains("no link in it to copy"));

        // Plain ↑ picks the older one; y copies what it says, l its link.
        key(&mut app, KeyCode::Up);
        assert_eq!(app.picked(), Some("m8"));
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(
            app.take_clipboard().as_deref(),
            Some("see https://zirc.dev/m/x.")
        );
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(row_with(&rows, "irc.zirc.test · ").contains("copied the message"));
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.take_clipboard().as_deref(),
            Some("https://zirc.dev/m/x")
        );
        assert_eq!(app.input.text(), "", "nothing typed");

        // Past the newest, nothing's picked, and keys type again.
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.picked(), None);
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(!row_with(&rows, "irc.zirc.test · ").contains("copies"));
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(app.input.text(), "y");
    }

    #[test]
    fn shows_a_sealed_part_locked_until_s_reveals_it() {
        use zirc_core::sealed::{Context, KEY_LEN, Key, seal_message};
        let key = Key::new("acme", [7; KEY_LEN]);
        let sealed = seal_message(
            &key,
            &Context { target: "#api" },
            &["sk_test_1".to_owned()],
            4094,
        )
        .unwrap();
        let mut app = app();
        let _ = app.select(1);
        app.chat.set_keys("acme", vec![key]);
        app.handle(
            format!(
                "@msgid=m8;time=2026-10-30T11:04:00.000Z;+zirc.dev/sealed={sealed} \
                 :bob!u@h PRIVMSG #api :the key is [sealed]"
            )
            .parse()
            .unwrap(),
        );
        let screen = Rect::new(0, 0, 120, 20);
        app.view = scrollback_size(&app, screen);
        // The lock takes two cells, the second drawn blank.
        let shown = |app: &App| {
            let (rows, _) = draw(app, screen.width, screen.height);
            row_with(&rows, "the key is").replace("🔒  ", "🔒 ")
        };
        assert!(
            shown(&app).contains("the key is 🔒 sealed"),
            "{}",
            shown(&app)
        );

        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        app.key(KeyEvent::from(KeyCode::Char('s')));
        assert!(
            shown(&app).contains("the key is 🔒 sk_test_1"),
            "{}",
            shown(&app)
        );
        app.key(KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(
            app.take_clipboard().as_deref(),
            Some("the key is 🔒 sk_test_1")
        );
        // Let go of, it hides again.
        app.key(KeyEvent::from(KeyCode::Esc));
        assert!(
            shown(&app).contains("the key is 🔒 sealed"),
            "{}",
            shown(&app)
        );
        assert!(!shown(&app).contains("sk_test_1"));
    }

    #[test]
    fn wont_send_a_part_it_cant_seal() {
        let mut app = app();
        let _ = app.select(1);
        for c in "the key is ||sk_test_1||".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let Action::Send(sent) = app.key(KeyEvent::from(KeyCode::Enter)) else {
            panic!("not a send")
        };
        assert!(
            sent.iter()
                .all(|line| !line.to_string().contains("sk_test_1")),
            "{sent:?}"
        );
        let (rows, _) = draw(&app, 120, 20);
        assert!(
            rows.iter()
                .any(|row| row.contains("not sent: this device has no key")),
            "{rows:?}"
        );
    }

    #[test]
    fn replies_to_the_message_picked_under_a_quote_of_it() {
        let mut app = app();
        let _ = app.select(1);
        app.handle(
            "@msgid=m7;time=2026-10-30T11:04:00.000Z :bob!u@h PRIVMSG #api :is staging up?"
                .parse()
                .unwrap(),
        );
        let screen = Rect::new(0, 0, 120, 20);
        app.view = scrollback_size(&app, screen);
        let key = |app: &mut App, code| app.key(KeyEvent::from(code));
        let sent = |action: Action| match action {
            Action::Send(lines) => lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };

        // Pick it, r: the prompt and the status line say what the next line answers.
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        assert_eq!(app.picked(), Some("m7"));
        key(&mut app, KeyCode::Char('r'));
        assert_eq!(app.picked(), None);
        let (rows, _) = draw(&app, screen.width, screen.height);
        // The prompt is the conversation's alone; the status line says what it answers, as
        // every app's bar words it.
        assert!(
            rows.iter().any(|row| row.trim_end() == "#api ›"),
            "{rows:?}"
        );
        let bar = format!(
            "{} bob: is staging up?, Esc cancels",
            zirc_core::chat::REPLYING_TO
        );
        assert!(
            row_with(&rows, "irc.zirc.test · ").contains(&bar),
            "{rows:?}"
        );

        // Esc lets the reply go; /reply with nothing starts it again, on the newest.
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.replying(), None);
        for c in "/reply".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.replying(), Some("m7"));
        for c in "it is".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        let lines = sent(key(&mut app, KeyCode::Enter));
        assert_eq!(
            lines,
            ["@+zirc.dev/client=tui;+draft/reply=m7 PRIVMSG #api :it is"]
        );
        assert_eq!(app.replying(), None, "only the next line answers it");

        // The server echoes it; it shows under its header, beneath a quote of what it
        // answers.
        app.handle(
            "@msgid=m8;+draft/reply=m7;time=2026-10-30T11:05:00.000Z :me!u@h PRIVMSG #api :it is"
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, screen.width, screen.height);
        let quote = rows
            .iter()
            .position(|row| row.contains("↪ bob: is staging up?"))
            .expect("quoted");
        assert!(
            rows[quote - 1].contains("me  11:05") && rows[quote + 1].contains("it is"),
            "{rows:?}"
        );

        // /reply with text answers the newest at once: our own, now.
        for c in "/reply and prod".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        let lines = sent(key(&mut app, KeyCode::Enter));
        assert_eq!(
            lines,
            ["@+zirc.dev/client=tui;+draft/reply=m8 PRIVMSG #api :and prod"]
        );
    }

    #[test]
    fn asks_before_trimming_a_message_and_says_what_it_reaches() {
        let mut app = app();
        let _ = app.select(1);
        app.handle(
            "@msgid=m9;time=2026-10-30T11:05:00.000Z :me!u@h PRIVMSG #api :the key is hunter2"
                .parse()
                .unwrap(),
        );
        let screen = Rect::new(0, 0, 110, 20);
        app.view = scrollback_size(&app, screen);
        for c in "/trim pasted by mistake".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter));
        let (rows, terminal) = draw(&app, screen.width, screen.height);
        let card = rows.join("\n");
        assert!(card.contains(" trim this message? "), "{card}");
        assert!(row_with(&rows, "<me> the key").contains("<me> the key is hunter2"));
        assert!(card.contains("because: pasted by mistake"), "{card}");
        assert!(card.contains("from Zirc's apps"), "{card}");
        assert!(card.contains("keep their copy"), "{card}");
        let (x, y) = place_of(&rows, "y trims it");
        assert_eq!(
            terminal.backend().buffer()[(x, y)].fg,
            colour(app.theme.warning)
        );

        // Kept: the card goes, and so does nothing else.
        app.key(KeyEvent::from(KeyCode::Esc));
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert!(!rows.join("\n").contains("trim this message?"));
        assert!(row_with(&rows, "the key is hunter2").contains("hunter2"));
    }

    #[test]
    fn says_when_we_are_away() {
        let mut app = app();
        let _ = app.select(1);
        app.handle(
            ":s 306 me :You have been marked as being away"
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, 100, 16);
        assert!(row_with(&rows, "irc.zirc.test · ").contains("· away"));
        app.handle(
            ":s 305 me :You are no longer marked as being away"
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, 100, 16);
        assert!(!row_with(&rows, "irc.zirc.test · ").contains(&away_label("")));
    }

    #[test]
    fn shows_scrollback_members_and_topic() {
        let mut app = app();
        app.chat
            .set_threshold("#api", zirc_core::chat::COMMITS_AND_PUSHES_SHOW_AT);
        let _ = app.select(1);
        let (rows, terminal) = draw(&app, 100, 16);
        assert!(
            rows[0].contains("#api") && rows[0].contains("the API"),
            "{}",
            rows[0]
        );
        assert!(row_with(&rows, "Today").contains("─ Today ─"));
        assert!(
            rows.iter().any(|row| row.contains("│ alice  11:00")),
            "{rows:#?}"
        );
        assert!(row_with(&rows, "morning").contains("│   morning"));
        // The second message from alice continues her run: no header.
        let mention = row_with(&rows, "can you look?");
        assert!(mention.contains("│ ▎ me: can you look?"), "{mention}");
        assert!(
            row_with(&rows, "committed")
                .contains("● cc-12 committed: Fix login  (main abc) · 11:02")
        );
        assert!(!rows.iter().any(|row| row.contains("ZIRC")), "no payloads");
        assert!(row_with(&rows, PEOPLE_SECTION).contains(&format!("│ {PEOPLE_SECTION}")));
        assert!(
            rows.iter().any(|row| row.contains("│ ● alice")),
            "{rows:#?}"
        );

        assert!(rows.last().unwrap().starts_with("#api ›"));

        // The mention is on the mention ground, its edge in front.
        let (y, x) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| row.find("can you look").map(|x| (y, x)))
            .unwrap();
        let x = u16::try_from(rows[y][..x].chars().count()).unwrap();
        let cell = &terminal.backend().buffer()[(x, u16::try_from(y).unwrap())];
        assert_eq!(cell.bg, colour(app.theme.mention_ground));

        // Someone away is quieter.
        app.handle(":alice!u@h AWAY :lunch".parse().unwrap());
        let (rows, terminal) = draw(&app, 100, 16);
        let y = rows
            .iter()
            .position(|row| row.contains("│ ◐ alice"))
            .unwrap();
        let x = u16::try_from(rows[y][..rows[y].find("alice").unwrap()].chars().count()).unwrap();
        let cell = &terminal.backend().buffer()[(x, u16::try_from(y).unwrap())];
        assert_eq!(cell.fg, colour(app.theme.muted));
    }

    #[test]
    fn shows_who_someone_is_over_the_scrollback() {
        let mut app = app();
        let _ = app.select(1);
        for text in [
            ":s 354 me 42 machine/mattias mattias-mbp H mattias-mbp :mattias",
            ":alice!u@h AWAY :lunch",
            ":s 330 me alice alice :is logged in as",
        ] {
            app.handle(text.parse().unwrap());
        }
        app.key(KeyEvent::from(KeyCode::Char('/')));
        for c in "whois alice".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.profile(), Some("alice"));
        let (rows, _) = draw(&app, 100, 20);
        let card = rows.join("\n");
        assert!(
            row_with(&rows, "alice away: lunch").contains("alice away: lunch"),
            "{card}"
        );
        assert!(card.contains("account alice"), "{card}");
        assert!(card.contains("in #api"), "{card}");
        assert!(card.contains("last said 11:01 here"), "{card}");
        assert!(
            card.contains(&format!(
                "Enter talks to them · {}",
                zirc_core::chat::labels::ESC_CLOSES
            )),
            "{card}"
        );

        // A machine's card says whose it is.
        app.key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.profile(), None);
        app.key(KeyEvent::from(KeyCode::Char('/')));
        for c in "whois mattias-mbp".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter));
        let (rows, _) = draw(&app, 100, 20);
        assert!(
            rows.iter().any(|row| row.contains("mattias · mattias-mbp")),
            "{}",
            rows.join("\n")
        );
    }

    #[test]
    fn keeps_the_cursor_in_view_while_typing() {
        let mut app = app();
        let _ = app.select(1);
        for c in "a long line that goes past the edge".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        let (rows, mut terminal) = draw(&app, 30, 10);
        assert!(
            rows.last().unwrap().ends_with("the edge"),
            "{}",
            rows.last().unwrap()
        );
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!((cursor.x, cursor.y), (29, 9));
    }

    #[test]
    fn shift_or_alt_enter_starts_a_new_line_and_the_input_grows_to_fit() {
        let sent = |action: Action| match action {
            Action::Send(lines) => lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
            _ => panic!("not sent"),
        };
        let mut app = app();
        let _ = app.select(1);
        let typed = |app: &mut App, text: &str| {
            for c in text.chars() {
                app.key(KeyEvent::from(KeyCode::Char(c)));
            }
        };
        typed(&mut app, "first");
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert!(sent(app.key(shift_enter)).is_empty());
        typed(&mut app, "second");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        typed(&mut app, "third");
        assert_eq!(app.input.text(), "first\nsecond\nthird");
        let (rows, mut terminal) = draw(&app, 60, 20);
        let input: Vec<&str> = rows[rows.len() - 3..]
            .iter()
            .map(|row| row.trim_end())
            .collect();
        assert_eq!(input, ["#api › first", "       second", "       third"]);
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!((cursor.x, cursor.y), (12, 19));
        // Enter sends all of it, as one message.
        let said = sent(app.key(KeyEvent::from(KeyCode::Enter))).join("\n");
        assert!(said.contains("first") && said.contains("third"), "{said}");
        assert!(app.input.text().is_empty());
    }

    #[test]
    fn talks_to_each_agent_on_our_machines_apart() {
        let mut app = app();
        let reply = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-1531f8",
            zirc_proto::Event::Text {
                body: "tests pass".into(),
            },
        ))
        .unwrap();
        app.handle(
            format!("@time=2026-10-30T11:05:00.000Z;msgid=r1 :me-laptop!u@machine/me PRIVMSG me :{reply}")
                .parse()
                .unwrap(),
        );
        let type_line = |app: &mut App, text: &str| -> Action {
            for c in text.chars() {
                app.key(KeyEvent::from(KeyCode::Char(c)));
            }
            app.key(KeyEvent::from(KeyCode::Enter))
        };
        let _ = type_line(&mut app, "/agents");
        let (rows, _) = draw(&app, 120, 16);
        row_with(&rows, "your agents, each to /query: me-laptop/cc-1531f8");

        let _ = type_line(&mut app, "/query me-laptop/cc-1531f8");
        let Action::Send(sent) = type_line(&mut app, "run them again") else {
            panic!("nothing to send");
        };
        assert_eq!(
            sent.last().unwrap().to_string(),
            "@+zirc.dev/client=tui PRIVMSG me-laptop :cc-1531f8 run them again"
        );
        let (rows, _) = draw(&app, 120, 16);
        assert!(row_with(&rows, AGENT_TAG).contains(&format!("cc-1531f8 {AGENT_TAG}")));
        row_with(&rows, "tests pass");
        assert!(
            rows.last().unwrap().starts_with("cc-1531f8 ›"),
            "{}",
            rows.last().unwrap()
        );
    }

    #[test]
    fn lists_each_direct_conversation_on_its_own_newest_first() {
        let mut app = app();
        let done = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-77aa01",
            zirc_proto::Event::Text {
                body: "done".into(),
            },
        ))
        .unwrap();
        for line in [
            format!(
                "@time=2026-10-30T11:00:00.000Z;msgid=a1 :me-laptop!u@machine/me PRIVMSG me :{done}"
            ),
            ":bob!u@h PRIVMSG me :hi".to_owned(),
            ":me!u@h JOIN #dm-abc".to_owned(),
            ":s 353 me @ #dm-abc :me alice".to_owned(),
        ] {
            app.handle(line.parse().unwrap());
        }
        let listed: Vec<String> = app.listed().map(|(_, c)| app.chat.title(&c.name)).collect();
        // Channels A–Z, then direct messages newest first, agent sessions among them.
        assert_eq!(listed, ["#api", "#web", "bob", "cc-77aa01", "alice"]);
        let screen = Rect::new(0, 0, 100, 16);
        // The list under the switch.
        let side = |rows: &[String], under: usize| -> Vec<String> {
            rows[2..2 + under]
                .iter()
                .map(|row| row.split('│').next().unwrap().trim_end().to_owned())
                .collect()
        };
        let (rows, _) = draw(&app, screen.width, screen.height);
        let channels = format!(" {}", Shelf::Channels.label());
        let direct = format!(" {}", Shelf::Direct.label());
        // The switch's Channels: A–Z, each with what was said last and when, where it fits.
        assert_eq!(
            side(&rows, 3),
            [
                // Unread in bold with a pip at the edge; a pill counts what mentions us.
                channels.as_str(),
                "▎#api cc-12: F… 58m 1",
                "▎#web bob: deploy… 57m",
            ]
        );
        // Its Messages: the direct messages newest first, every conversation on its own, our
        // agent's among them as it wrote to us.
        app.chat.set_tab(zirc_core::chat::SidebarSection::Direct);
        let (rows, _) = draw(&app, screen.width, screen.height);
        assert_eq!(
            side(&rows, 4),
            [
                direct.as_str(),
                // What a person last said, dimmed, and when; their presence in front. Unread,
                // with no pip: a direct message never has one (the user's choice, 20 Nov 2026).
                " ● bob hi       now 1",
                // A session by what it goes by, its tool's mark in front in its colour; the
                // list too narrow here for its preview.
                " ⚙ cc-77aa01        1",
                " alice",
            ]
        );
        app.chat.set_tab(zirc_core::chat::SidebarSection::Direct);
        let (rows, buffer) = draw(&app, screen.width, screen.height);
        let (x, y) = place_of(&rows, "hi       now");
        assert_eq!(
            buffer.backend().buffer()[(x, y)].fg,
            colour(app.theme.muted)
        );
        let (x, y) = place_of(&rows, "● bob hi");
        assert_eq!(
            buffer.backend().buffer()[(x, y)].fg,
            colour(app.theme.presence_here)
        );
        assert!(
            buffer.backend().buffer()[(x + 2, y)]
                .modifier
                .contains(Modifier::BOLD)
        );
        // Muted, it's in the muted colour and never bold, unread or not.
        app.chat.mute("bob", Some(zirc_core::chat::Muted::Off));
        let buffer = draw(&app, screen.width, screen.height).1;
        let cell = &buffer.backend().buffer()[(x + 2, y)];
        assert_eq!(cell.fg, colour(app.theme.muted));
        assert!(!cell.modifier.contains(Modifier::BOLD));
        // And no pip.
        assert_eq!(buffer.backend().buffer()[(x - 1, y)].symbol(), " ");
        // Our agent's session, among the direct messages, as it wrote to us.
        let (rows, _) = draw(&app, screen.width, screen.height);
        let (x, y) = place_of(&rows, "⚙ cc-77aa01");
        assert_ne!(
            draw(&app, screen.width, screen.height).1.backend().buffer()[(x, y)].fg,
            colour(app.theme.muted)
        );
        let (x, y) = place_of(&rows, "cc-77aa01");
        let target = target_at(&app, screen, x, y);
        assert!(
            matches!(target, Some(Target::Conversation(_))),
            "{target:?}"
        );
        let _ = app.pointer(
            ratatui::crossterm::event::MouseEventKind::Down(
                ratatui::crossterm::event::MouseButton::Left,
            ),
            target,
        );
        assert_eq!(app.current().name, "me-laptop/cc-77aa01");
    }

    #[test]
    fn heads_the_list_with_the_org_and_leaves_agents_that_never_wrote_to_us_out() {
        let mut app = app();
        app.chat.set_org_name(Some("Acme"));
        let taking = |session: &str, task: &str| {
            zirc_proto::encode(&zirc_proto::Message::new(
                session,
                zirc_proto::Event::Text {
                    body: format!("taking: {task}"),
                },
            ))
            .unwrap()
        };
        for line in [
            ":bob!u@h PRIVMSG me :hi".to_owned(),
            ":bob-laptop!u@machine/bob JOIN #api".to_owned(),
            format!(
                "@time=2026-10-30T11:15:00.000Z :me-laptop!u@machine/me PRIVMSG #api :{}",
                taking("cc-1", "CI")
            ),
            format!(
                "@time=2026-10-30T11:30:00.000Z :bob-laptop!u@machine/bob PRIVMSG #api :{}",
                taking("cc-9", "billing")
            ),
        ] {
            app.handle(line.parse().unwrap());
        }
        let screen = Rect::new(0, 0, 100, 16);
        app.chat.set_tab(zirc_core::chat::SidebarSection::Direct);
        let (rows, _) = draw(&app, screen.width, screen.height);
        let side: Vec<&str> = rows
            .iter()
            .map(|row| row.split('│').next().unwrap().trim_end())
            .collect();
        // The organisation and its people on top, as zirc-chat says them.
        let people = app.chat.org_header().members;
        assert_eq!(side[1], format!(" Acme · {people} members"));
        // Agents at work that only posted in a channel aren't direct messages (the user's
        // choice, 15 Nov 2026): no row of theirs, and no Machines.
        assert!(
            !side
                .iter()
                .any(|row| row.contains("cc-1") || row.contains("cc-9") || row.contains(MACHINES)),
            "{side:?}"
        );
        assert!(side.iter().any(|row| row.contains("bob")), "{side:?}");
    }

    #[test]
    fn folds_a_section_and_browses_channels_from_the_list() {
        let mut app = app();
        app.handle(":me!u@h JOIN #quiet".parse().unwrap());
        let _ = app.select(app.chat.position("#api").unwrap());
        let screen = Rect::new(0, 0, 100, 16);
        let click = ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Left,
        );
        let (rows, _) = draw(&app, screen.width, screen.height);
        row_with(&rows, "#quiet");
        // A click on Channels folds it away: only what's new in it, or shown, stays, and its
        // heading says there's more.
        let (x, y) = heading_at(&rows, Shelf::Channels.label());
        let _ = app.pointer(click, target_at(&app, screen, x, y));
        let (rows, _) = draw(&app, screen.width, screen.height);
        row_with(&rows, " Channels ▸");
        row_with(&rows, "#api");
        let gone = ["#quiet"];
        assert!(
            !rows
                .iter()
                .any(|row| gone.iter().any(|name| row.contains(name))),
            "{rows:?}"
        );
        // And a click opens it again.
        let (x, y) = place_of(&rows, " Channels ▸");
        let _ = app.pointer(click, target_at(&app, screen, x + 1, y));
        let (rows, _) = draw(&app, screen.width, screen.height);
        row_with(&rows, "#quiet");
    }

    #[test]
    fn says_how_much_is_new_and_since_when() {
        let mut app = app();
        let _ = app.select(1);
        for line in [
            "@msgid=w1;time=2026-10-30T11:40:00.000Z :bob!u@h PRIVMSG #web :one",
            "@msgid=w2;time=2026-10-30T11:41:00.000Z :bob!u@h PRIVMSG #web :two",
        ] {
            app.handle(line.parse().unwrap());
        }
        let _ = app.select(app.chat.position("#web").unwrap());
        let (rows, _) = draw(&app, 100, 24);
        // The New divider says it, as zirc-chat counts it.
        assert_eq!(
            app.chat
                .unseen(|at| app.zone.wall(at))
                .map(|unseen| unseen.count),
            Some(2)
        );
        row_with(&rows, "──── 2 new messages since 11:40  NEW  │");
        // Narrow, its short words.
        let (rows, _) = draw(&app, 28, 24);
        row_with(&rows, "─ ↓ 2 new");
    }

    #[test]
    fn browses_channels_over_the_scrollback_and_asks_before_leaving() {
        let mut app = app();
        let _ = app.select(app.chat.position("#api").unwrap());
        let typing = |app: &mut App, text: &str| {
            for c in text.chars() {
                app.key(KeyEvent::from(KeyCode::Char(c)));
            }
            app.key(KeyEvent::from(KeyCode::Enter));
        };
        typing(&mut app, "/browse");
        let (rows, _) = draw(&app, 100, 24);
        row_with(&rows, BROWSE_CHANNELS);
        row_with(&rows, NO_OTHER_CHANNELS);
        for line in [
            ":s 322 me #api/checkout 3 :",
            ":s 322 me #api/login 1 :",
            ":s 323 me :End of LIST",
        ] {
            app.handle(line.parse().unwrap());
        }
        let (rows, _) = draw(&app, 100, 24);
        row_with(&rows, &format!("#api/checkout  {}", members_label(3)));
        row_with(&rows, &format!("#api/login     {}", members_label(1)));
        app.key(KeyEvent::from(KeyCode::Esc));
        // Leaving asks on the line over the input, the keys in short where they don't fit.
        typing(&mut app, "/part");
        let (rows, _) = draw(&app, 120, 24);
        row_with(
            &rows,
            &format!(
                " {}  y leaves · any other key",
                app.chat.leave_prompt("#api")
            ),
        );
        let (rows, _) = draw(&app, 80, 24);
        let asked = row_with(&rows, " Leave #api?");
        assert!(asked.trim_end().ends_with("channels.  y leaves"), "{asked}");
    }

    #[test]
    fn says_once_two_sessions_are_changing_a_file_and_shows_it_on_who() {
        let mut app = app();
        let _ = app.select(app.chat.position("#api").unwrap());
        // bob-mbp's working sets, one snapshot in parts: checkout's, then rate-limits'.
        let snapshot = |limits: &[&str]| {
            let sets = [
                ("cc-1", "checkout", vec!["web/src/api/client.ts"]),
                ("cc-2", "rate-limits", limits.to_vec()),
            ];
            sets.into_iter()
                .zip(1..)
                .map(|((session, name, paths), part)| {
                    let mut message = zirc_proto::Message::new(
                        session,
                        zirc_proto::Event::Working {
                            paths: paths.iter().map(|&p| p.to_owned()).collect(),
                            continued: part > 1,
                            worktree: Some(name.into()),
                            branch: Some(name.into()),
                            part: Some([part, 2]),
                            base: None,
                            committed: Vec::new(),
                        },
                    );
                    message.name = Some(name.into());
                    zirc_core::presence::line_from("bob-mbp", "#api", &message).unwrap()
                })
                .collect::<Vec<_>>()
        };
        app.handle(":bob-mbp!u@machine/bob JOIN #api".parse().unwrap());
        for line in snapshot(&["web/src/limits.ts"])
            .into_iter()
            .chain(snapshot(&["web/src/limits.ts", "web/src/api/client.ts"]))
        {
            app.handle(line);
        }
        // The stream says it once, as a message: the `!`, who else, then the path listed
        // under it, as every event's are (the user's choice, 14 Nov 2026).
        let (rows, _) = draw(&app, 120, 36);
        let said = row_with(&rows, "! checkout is also changing");
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("also changing"))
                .count(),
            1,
            "{rows:#?}"
        );
        assert!(!said.contains("·") && !said.contains("client.ts"), "{said}");
        let at = rows
            .iter()
            .position(|row| row.contains("! checkout is also changing"));
        assert!(
            rows[at.unwrap() + 1].contains("web/src/api/client.ts"),
            "{rows:#?}"
        );

        // Its card: Now, then what it's changing with others.
        let typing = |app: &mut App, text: &str| {
            for c in text.chars() {
                app.key(KeyEvent::from(KeyCode::Char(c)));
            }
            app.key(KeyEvent::from(KeyCode::Enter));
        };
        typing(&mut app, "/who rate-limits");
        let (rows, _) = draw(&app, 80, 24);
        row_with(&rows, "│ rate-limits");
        row_with(&rows, "! also changing web/src/api/client.ts with checkout");
        app.key(KeyEvent::from(KeyCode::Esc));
        assert!(app.agent().is_none());
        typing(&mut app, "/who nobody");
        let (rows, _) = draw(&app, 80, 24);
        row_with(&rows, "No agent session goes by nobody here");
    }

    #[test]
    fn runs_an_agents_posts_on_and_chips_the_people_named() {
        let mut app = app();
        let post = |body: &str| {
            let mut message = zirc_proto::Message::new(
                "cc-1531f8",
                zirc_proto::Event::Text { body: body.into() },
            );
            message.name = Some("checkout".into());
            zirc_proto::encode(&message).unwrap()
        };
        for line in [
            format!(
                "@time=2026-10-30T10:50:00.000Z :me-laptop!u@machine/me PRIVMSG me :{}",
                post("on it")
            ),
            format!(
                "@time=2026-10-30T10:51:00.000Z :me-laptop!u@machine/me PRIVMSG me :{}",
                post("fixed; ask @alice to review?")
            ),
        ] {
            app.handle(line.parse().unwrap());
        }
        let _ = app.select(app.chat.position("me-laptop/cc-1531f8").unwrap());
        let (rows, terminal) = draw(&app, 120, 20);
        // One header, as a person's run has.
        assert_eq!(rows.iter().filter(|row| row.contains("  10:5")).count(), 1);
        row_with(&rows, "on it");
        // A person named is a chip, with no "@".
        let (x, y) = place_of(&rows, "fixed; ask alice to review?");
        let cell = &terminal.backend().buffer()[(x + 11, y)];
        assert_eq!(cell.symbol(), "a");
        assert_eq!(cell.bg, colour(app.theme.mention_chip_ground));
    }

    #[test]
    fn says_an_agents_taking_on_in_a_channel_as_one_quiet_line() {
        let mut app = app();
        let _ = app.select(1);
        let taking = zirc_proto::encode(&zirc_proto::Message::new(
            "cc-1531f8",
            zirc_proto::Event::Text {
                body:
                    "taking: fix the coupon test that fails on staging and on the nightly build too"
                        .into(),
            },
        ))
        .unwrap();
        app.handle(
            format!(
                "@time=2026-10-30T11:40:00.000Z :me-laptop!u@machine/me PRIVMSG #api :{taking}"
            )
            .parse()
            .unwrap(),
        );
        let (rows, _) = draw(&app, 100, 16);
        // In a channel, one quiet line in the core's words, its time at the end (the user's
        // choice, 14 Nov 2026): never a header and body.
        assert!(row_with(&rows, "is taking: fix the coupon").contains("11:40"));
        assert_eq!(
            rows.iter().filter(|row| row.contains("coupon")).count(),
            1,
            "{rows:#?}"
        );
    }

    #[test]
    fn starts_an_empty_channel_and_dims_rather_than_empties_offline() {
        let mut app = app();
        app.handle(":me!u@h JOIN #empty".parse().unwrap());
        let _ = app.select(app.chat.position("#empty").unwrap());
        let (rows, _) = draw(&app, 100, 16);
        // How it starts, as zirc-chat says it.
        row_with(&rows, "This is the start of #empty.");

        let _ = app.select(1);
        app.disconnected("connection reset by peer");
        let (rows, terminal) = draw(&app, 100, 16);
        // A warning line over the scrollback, as zirc-core words it; who was here stays,
        // quieter.
        row_with(&rows, "Reconnecting…");
        let (x, y) = place_of(&rows, "● bob");
        let cell = &terminal.backend().buffer()[(x + 2, y)];
        assert_eq!(cell.symbol(), "b");
        assert_eq!(cell.fg, colour(app.theme.muted));
        assert!(cell.modifier.contains(Modifier::DIM));
    }

    #[test]
    fn keeps_the_selection_while_offline() {
        let mut app = app();
        let _ = app.select(app.chat.position("#api").unwrap());
        app.disconnected("connection reset by peer");
        let (rows, terminal) = draw(&app, 100, 16);
        // The one shown keeps its ground and isn't dimmed with the rest; the list is
        // under the header.
        let list = &rows[1..];
        let cell = |text: &str| {
            let (x, y) = place_of(list, text);
            terminal.backend().buffer()[(x + 1, y + 1)].clone()
        };
        let api = cell("#api");
        assert_eq!(api.symbol(), "a");
        assert_eq!(api.bg, colour(app.theme.selected));
        assert!(!api.modifier.contains(Modifier::DIM));
        assert!(cell("#web").modifier.contains(Modifier::DIM));
    }

    #[test]
    fn zircs_system_account_is_zirc_and_its_messages_take_no_replies() {
        let mut app = app();
        app.handle(
            ":zirc-system!u@h NOTICE #api :#api is now public"
                .parse()
                .unwrap(),
        );
        let _ = app.select(app.chat.position("#api").unwrap());
        let (rows, _) = draw(&app, 120, 16);
        // Headed by its name, Zirc, as a message is.
        let at = rows
            .iter()
            .position(|row| row.contains("#api is now public"))
            .unwrap();
        assert!(rows[at - 1].contains("Zirc"), "{rows:?}");
        assert!(
            !rows.iter().any(|row| row.contains("zirc-system")),
            "{rows:?}"
        );

        app.handle(
            ":zirc-system!u@h PRIVMSG me :Your trial ends in 3 days"
                .parse()
                .unwrap(),
        );
        let _ = app.select(app.chat.position("zirc-system").unwrap());
        let (rows, _) = draw(&app, 120, 16);
        // Its input says it takes no replies, and what's typed there isn't sent.
        row_with(&rows, "Zirc › Zirc's notices can't be answered.");
        for c in "thanks".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(
            app.key(KeyEvent::from(KeyCode::Enter)),
            Action::Send(Vec::new())
        );
    }

    #[test]
    fn says_a_failed_send_in_one_line_and_gives_back_what_was_typed() {
        let mut app = app();
        let _ = app.select(app.chat.position("#api").unwrap());
        for c in "on it".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter));
        app.unsent("you're offline");
        assert_eq!(app.input.text(), "on it");
        let (rows, terminal) = draw(&app, 130, 16);
        let words = "Not sent, you're offline: it's back in the input, Enter sends it again";
        // One quiet line, as zirc-chat shapes every note for every app (RowShape::Quiet).
        let (x, y) = place_of(&rows, words);
        assert_eq!(
            terminal.backend().buffer()[(x, y)].fg,
            colour(app.theme.muted)
        );

        // What a key sends, not what was typed, has nothing to give back.
        let _ = app.input.take();
        app.key(KeyEvent::from(KeyCode::Esc));
        app.unsent("you're offline");
        assert_eq!(app.input.text(), "");

        // A command mistyped stays, to put right.
        for c in "/nope".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.input.text(), "/nope");
        let (rows, _) = draw(&app, 100, 16);
        row_with(&rows, "no command /nope: /help lists them");
    }

    #[test]
    fn nests_branch_rooms_and_pins_to_the_top() {
        let mut app = app();
        for line in [
            ":me!u@h JOIN #api/login",
            "@time=2026-10-30T11:30:00.000Z :bob!u@h PRIVMSG #api/login :rebased",
        ] {
            app.handle(line.parse().unwrap());
        }
        let side = |app: &App| {
            let (rows, _) = draw(app, 100, 16);
            // Under the switch.
            rows[2..6]
                .iter()
                .map(|row| row.split('│').next().unwrap().trim_end().to_owned())
                .collect::<Vec<_>>()
        };
        let channels = format!(" {}", Shelf::Channels.label());
        let starred = format!(" {}", Shelf::Starred.label());
        // The room under its repository's channel, the channels A–Z; the channel previews
        // its own words, not its room's.
        assert_eq!(
            side(&app),
            [
                channels.as_str(),
                "▎#api cc-12:… 58m 1",
                "   └ login",
                "▎#web bob: depl… 57m"
            ]
        );

        // Alt-P pins the conversation shown, under Starred at the top; it's said, and kept.
        let _ = app.select(app.chat.position("#web").unwrap());
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
        assert_eq!(
            side(&app),
            [
                starred.as_str(),
                // Not marked: it's under Starred.
                " #web bob: depl… 57m",
                channels.as_str(),
                "▎#api cc-12:… 58m 1"
            ]
        );
        let (rows, _) = draw(&app, 100, 16);
        assert!(row_with(&rows, "irc.zirc.test · ").contains("pinned #web, Alt-P unpins"));
        assert_eq!(app.take_prefs().unwrap().pins, ["#web"]);
        assert_eq!(app.take_prefs(), None, "taken once");
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
        assert!(app.take_prefs().unwrap().pins.is_empty());

        // Pins kept from before are taken up.
        app.use_prefs(crate::prefs::Prefs {
            pins: vec!["#web".into()],
            ..crate::prefs::Prefs::default()
        });
        assert_eq!(side(&app)[0], starred);
        assert!(side(&app)[1].starts_with(" #web "));
        // The channel's header counts its threads, in the core's line.
        let _ = app.select(app.chat.position("#api").unwrap());
        assert!(
            draw(&app, 100, 16).0[0].contains("1 thread"),
            "{:?}",
            draw(&app, 100, 16).0[0]
        );
    }

    #[test]
    fn says_who_is_typing_and_that_we_are() {
        let mut app = app();
        let _ = app.select(app.chat.position("#api").unwrap());
        let key = |app: &mut App, code| match app.key(KeyEvent::from(code)) {
            Action::Send(lines) => lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };

        // Someone typing shows on the line under the input.
        app.handle("@+typing=active :bob!u@h TAGMSG #api".parse().unwrap());
        let (rows, _) = draw(&app, 100, 16);
        assert_eq!(rows[rows.len() - 1].trim_end(), " bob is typing…");
        app.handle("@+typing=active :alice!u@h TAGMSG #api".parse().unwrap());
        let (rows, _) = draw(&app, 100, 16);
        assert_eq!(
            rows[rows.len() - 1].trim_end(),
            " bob and alice are typing…"
        );
        // What they say, or saying they're done, ends it.
        app.handle(":bob!u@h PRIVMSG #api :done".parse().unwrap());
        app.handle("@+typing=done :alice!u@h TAGMSG #api".parse().unwrap());
        let (rows, _) = draw(&app, 100, 16);
        assert!(!rows.iter().any(|row| row.contains("typing")), "{rows:#?}");

        // One of our agent sessions at work is thinking, in its own conversation.
        let _ = app.chat.open("me-laptop/cc-1531f8");
        app.handle(
            "@+typing=active;+zirc.dev/session=cc-1531f8 :me-laptop!u@machine/me TAGMSG me"
                .parse()
                .unwrap(),
        );
        let (rows, _) = draw(&app, 100, 16);
        assert_eq!(rows[rows.len() - 1].trim_end(), " cc-1531f8 is thinking…");
        let _ = app.select(app.chat.position("#api").unwrap());

        // We type: said once, not every key; clearing it says we're done. Not commands.
        assert_eq!(
            key(&mut app, KeyCode::Char('h')),
            ["@+typing=active TAGMSG #api"]
        );
        assert!(key(&mut app, KeyCode::Char('i')).is_empty());
        assert!(key(&mut app, KeyCode::Backspace).is_empty());
        assert_eq!(
            key(&mut app, KeyCode::Backspace),
            ["@+typing=done TAGMSG #api"]
        );
        assert!(key(&mut app, KeyCode::Char('/')).is_empty());
        key(&mut app, KeyCode::Backspace);

        // /typing off stops it, and is kept.
        for c in "/typing off".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter);
        assert!(!app.take_prefs().unwrap().send_typing);
        assert!(key(&mut app, KeyCode::Char('h')).is_empty());
    }

    #[test]
    fn heads_runs_with_who_and_when() {
        let mut app = app();
        app.chat
            .set_threshold("#api", zirc_core::chat::COMMITS_AND_PUSHES_SHOW_AT);
        let _ = app.select(app.chat.position("#api").unwrap());
        let screen = Rect::new(0, 0, 100, 16);
        app.view = scrollback_size(&app, screen);
        let scrollback = |app: &App| {
            let (rows, _) = draw(app, screen.width, screen.height);
            rows.iter()
                .filter_map(|row| Some(row.split('│').nth(1)?.trim_end().to_owned()))
                .filter(|row| !row.trim().is_empty())
                .collect::<Vec<_>>()
        };
        // The time after the name, and alice's second message under the first's header; an
        // agent's commit a message too: its header, who and when, then its lead and what it
        // was.
        let rows = scrollback(&app);
        assert!(rows.len() >= 4, "{rows:#?}");
        assert_eq!(
            rows[rows.len() - 4..],
            [
                " alice  11:00",
                "   morning",
                // What mentions us, its edge in front.
                " ▎ me: can you look?",
                // An agent's commit in a channel: one quiet line in the core's words.
                " ⚙ ● cc-12 committed: Fix login  (main abc) · 11:02",
            ]
        );

        // The message picked has the bar in front.
        app.handle(
            "@msgid=m1;time=2026-10-30T11:05:00.000Z :bob!u@h PRIVMSG #api :looking"
                .parse()
                .unwrap(),
        );
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        let rows = scrollback(&app);
        assert_eq!(rows[rows.len() - 2..], [" bob  11:05", " ▌ looking"]);
    }

    #[test]
    fn shows_what_every_session_is_doing() {
        use zirc_core::sessions::{Landing, Session};
        let mut app = app();
        let _ = app.select(app.chat.position("#api").unwrap());
        // Alt-S, or /sessions, asks zircd and opens the panel meanwhile.
        let asked = app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
        assert_eq!(asked, Action::Sessions);
        let (rows, _) = draw(&app, 120, 20);
        row_with(&rows, "asking zircd…");

        let now = 1_793_618_707;
        let session = Session {
            name: "cc-1531f8".into(),
            machine: Some("mattias-mbp".into()),
            channel: "#api".into(),
            task: Some("the login bug".into()),
            intent: None,
            branch: Some("login".into()),
            base: None,
            changed: Vec::new(),
            landing: Some(Landing::Queued {
                branch: "login".into(),
                build: Some(12),
            }),
            waiting: Some("its landing, build 12".into()),
            heard: Some(now - 180),
        };
        app.sessions_answered(Ok(vec![session]), now);
        let (rows, _) = draw(&app, 120, 20);
        let card = rows.join("\n");
        assert!(card.contains(" sessions "), "{card}");
        for line in [
            "cc-1531f8 on mattias-mbp · login · 3m ago",
            "  taking: the login bug",
            "  landing: landing login in build 12",
            "  waiting on: its landing, build 12",
            zirc_core::chat::labels::ESC_CLOSES,
        ] {
            assert!(card.contains(line), "{line} in\n{card}");
        }

        // Esc closes it; zircd that can't be asked says why.
        app.key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.sessions(), None);
        for c in "/sessions".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(app.key(KeyEvent::from(KeyCode::Enter)), Action::Sessions);
        app.sessions_answered(Err("zircd isn't running".into()), now);
        let (rows, _) = draw(&app, 120, 20);
        row_with(&rows, "zircd: zircd isn't running");
    }

    #[test]
    fn esc_marks_the_conversation_read_and_shift_esc_every_one() {
        let mut app = app();
        let api = app.chat.position("#api").unwrap();
        let web = app.chat.position("#web").unwrap();
        let unread = |app: &App| {
            let conversations = app.conversations();
            (conversations[api].unread, conversations[web].unread)
        };
        // The commit in #api, shown only under Everything, isn't unread.
        assert_eq!(unread(&app), (2, 1));
        // Esc, with nothing picked, marks only the conversation shown read: the server's.
        app.key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(unread(&app), (2, 1));
        let _ = app.select(api);
        app.handle(
            "@msgid=m1;time=2026-10-30T11:59:00.000Z :bob!u@h PRIVMSG #web :more"
                .parse()
                .unwrap(),
        );
        assert_eq!(unread(&app), (0, 2));
        // Shift-Esc marks every one read.
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT));
        assert_eq!(unread(&app), (0, 0));
    }

    #[test]
    fn narrow_terminals_drop_the_side_panels() {
        let mut app = app();
        let _ = app.select(1);
        let (rows, _) = draw(&app, 40, 10);
        // No people beside it, nor the list (the header names how many, as the core says).
        assert!(!rows.iter().any(|row| row.contains(PEOPLE_SECTION)));
        assert!(!rows.iter().any(|row| row.contains("#web")));
        assert!(row_with(&rows, "alice").contains("alice"));
    }

    #[test]
    fn draws_in_zircs_colours_dark_or_light() {
        for theme in [Theme::dark(), Theme::light()] {
            let mut app = app();
            app.chat
                .set_threshold("#api", zirc_core::chat::COMMITS_AND_PUSHES_SHOW_AT);
            app.theme = theme;
            let _ = app.select(1);
            let (rows, terminal) = draw(&app, 100, 16);
            let buffer = terminal.backend().buffer();
            let cell = |text: &str| {
                let (x, y) = place_of(&rows, text);
                buffer[(x, y)].clone()
            };
            // Everything on the terminal's own background, which the theme suits.
            assert_eq!(cell("morning").bg, Color::Reset);
            assert_eq!(cell("morning").fg, colour(theme.body));
            assert_eq!(cell("bob").bg, Color::Reset);
            // The conversation shown on the selected ground, its name whole, "#" and all;
            // what's unread in bold text, with a pip at the edge.
            let (x, y) = place_of(&rows[1..], " #api");
            assert_eq!(buffer[(x + 1, y + 1)].bg, colour(theme.selected));
            assert_eq!(buffer[(x + 1, y + 1)].symbol(), "#");
            assert_eq!(cell("#web").fg, colour(theme.text));
            assert!(cell("#web").modifier.contains(Modifier::BOLD));
            let (x, y) = place_of(&rows, "#web");
            assert_eq!(buffer[(x - 1, y)].symbol(), "▎");
            // Where we are, on the status line, the organisation quieter than the
            // conversation.
            assert_eq!(cell("irc.zirc.test · #api").fg, colour(theme.muted));
            assert_eq!(cell("● cc-12").fg, colour(theme.muted));
            assert_eq!(cell("● bob").fg, colour(theme.presence_here));
        }
    }

    #[test]
    fn names_the_organisation_and_whats_unread_in_the_others_only_with_several() {
        let mut app = app();
        let (rows, _) = draw(&app, 120, 20);
        assert!(
            !rows.iter().any(|row| row.contains("Alt-O")),
            "{}",
            rows.join("\n")
        );
        app.org = Some("acme".into());
        app.orgs = vec![
            (
                "acme".into(),
                zirc_core::orgs::Unread {
                    unread: 2,
                    mentions: 1,
                },
            ),
            (
                "globex".into(),
                zirc_core::orgs::Unread {
                    unread: 3,
                    mentions: 1,
                },
            ),
            ("public".into(), zirc_core::orgs::Unread::default()),
        ];
        let (rows, _) = draw(&app, 160, 20);
        let status = row_with(&rows, "acme · ");
        assert!(
            status.contains(&format!("globex 3 ({}), Alt-O", mentions_label(1))),
            "{status}"
        );
        assert!(!status.contains("public"), "{status}");
    }
}
