//! zirc's terminal IRC client: a chat client for an organisation's server, with its own
//! connection, as the person rather than the machine.
//!
//! The logic lives apart from the terminal so it can be tested without one: the chat
//! itself is zirc-core's [`zirc_core::chat`], shared with zirc's other apps; [`app`] adds
//! keys and commands to it, [`rows`] lays a conversation out, and [`ui`] only draws. [`session`] connects them to the server, and [`run`] to the terminal.

pub mod app;
pub mod cli;
pub mod command;
pub mod hyperlink;
pub mod input;
pub mod jump;
pub mod picker;
pub mod prefs;
pub mod rows;
pub mod scrollback;
pub mod session;
pub mod signin;
pub mod ui;

use std::io::Write as _;

use anyhow::Result;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;

pub use app::App;
pub use cli::Cli;
pub use session::{Session, Step};
use zirc_core::orgs::{Orgs, Unread};

/// How often the screen is drawn again while offline.
const COUNTDOWN: std::time::Duration = std::time::Duration::from_secs(1);

/// `text` as an OSC 52 sequence, which has the terminal put it on the clipboard, over SSH
/// too.
#[must_use]
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

/// Connects to each organisation, then runs the client in the terminal until it's quit, with
/// the `mouse` too when asked, changing this machine's settings through the zircd listening
/// on `zircd` and keeping the person's prefs at `prefs`. Each of `orgs` is an organisation,
/// its app and its connection; the first is shown first.
///
/// # Errors
///
/// When a first connection fails, or the terminal can't be drawn on.
pub async fn run(
    orgs: Vec<(String, App, zirc_irc::Config)>,
    mouse: bool,
    zircd: std::path::PathBuf,
    prefs: std::path::PathBuf,
) -> Result<()> {
    // Connect before taking over the terminal, so a failure reads plainly.
    let several = orgs.len() > 1;
    let mut sessions = Orgs::new();
    for (org, app, config) in orgs {
        eprintln!("Connecting to {}:{}…", config.host, config.port);
        let mut session = Session::connect(app, config)
            .await
            .map_err(anyhow::Error::from)
            .map_err(|err| {
                if several {
                    err.context(org.clone())
                } else {
                    err
                }
            })?;
        session.zircd = Some(zircd.clone());
        session.ask_checkouts();
        sessions.insert(org, session);
    }
    let mut terminal = ratatui::init();
    if mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    // Where the terminal can say so, Shift+Enter apart from Enter, for a new line in the
    // input; elsewhere Alt+Enter does it.
    let enhanced = ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
    let result = drive(&mut terminal, &mut sessions, &prefs).await;
    if enhanced {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result?;
    let names: Vec<String> = sessions.iter().map(|(org, _)| org.to_owned()).collect();
    for org in names {
        if let Some(session) = sessions.remove(&org) {
            session.finish().await;
        }
    }
    Ok(())
}

async fn drive(
    terminal: &mut ratatui::DefaultTerminal,
    orgs: &mut Orgs<Session>,
    prefs: &std::path::Path,
) -> Result<()> {
    let (keys, mut events) = tokio::sync::mpsc::unbounded_channel();
    // crossterm reads the terminal blocking; a thread hands its events over.
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if keys.send(event).is_err() {
                break;
            }
        }
    });
    loop {
        tell_orgs(orgs);
        let size = terminal.size()?;
        let screen = Rect::new(0, 0, size.width, size.height);
        let Some((_, session)) = orgs.selected_mut() else {
            return Ok(());
        };
        session.app.view = ui::scrollback_size(&session.app, screen);
        terminal.draw(|frame| ui::render(frame, &session.app))?;
        let waiting = !session.app.connection.is_connected() || session.app.typing().is_some();
        tokio::select! {
            event = events.recv() => {
                let Some((_, session)) = orgs.selected_mut() else {
                    return Ok(());
                };
                match event {
                    Some(Event::Key(key)) => {
                        match session.key(key) {
                            Step::Carry => {}
                            Step::Quit => return Ok(()),
                            Step::Org(name) => {
                                switch(orgs, name.as_deref());
                                continue;
                            }
                        }
                        if let Some(changed) = session.app.take_prefs() {
                            // Not worth stopping for: the next change tries again.
                            let _ = changed.save(prefs);
                        }
                        if let Some(text) = session.app.take_clipboard() {
                            let mut out = std::io::stdout();
                            out.write_all(osc52(&text).as_bytes())?;
                            out.flush()?;
                        }
                    }
                    Some(Event::Mouse(mouse)) => {
                        let target = ui::target_at(&session.app, screen, mouse.column, mouse.row);
                        session.pointer(mouse.kind, target);
                    }
                    Some(_) => {}
                    None => return Ok(()),
                }
            }
            () = network(orgs) => {}
            // Offline, the status line counts down to the next attempt; and who's typing
            // stops showing once they've stopped saying so.
            () = tokio::time::sleep(COUNTDOWN), if waiting => {}
        }
    }
}

/// Waits for the next thing from any organisation's connection, and takes it in, as
/// [`Session::network`] does. Safe to cancel, as each organisation's is.
pub async fn network(orgs: &mut Orgs<Session>) {
    std::future::poll_fn(|cx| {
        for (_, session) in orgs.iter_mut() {
            let next = std::pin::pin!(session.network());
            if next.poll(cx).is_ready() {
                return std::task::Poll::Ready(());
            }
        }
        std::task::Poll::Pending
    })
    .await;
}

/// Tells each organisation's app which organisations there are and what's unread in each,
/// for its status line, and whether it's the one shown: nothing, with one.
pub fn tell_orgs(orgs: &mut Orgs<Session>) {
    let shown = orgs.selected().map(|(org, _)| org.to_owned());
    if orgs.len() > 1 {
        for (org, session) in orgs.iter_mut() {
            session.hide(shown.as_deref() != Some(org));
        }
    }
    let unread: Vec<(String, Unread)> = if orgs.len() > 1 {
        // Each counted as every app counts an organisation's badge (zirc-chat's org_badge).
        orgs.iter()
            .map(|(org, session)| {
                let chat: &zirc_core::chat::Chat = session.as_ref();
                (org.to_owned(), chat.org_badge())
            })
            .collect()
    } else {
        Vec::new()
    };
    let several = !unread.is_empty();
    for (org, session) in orgs.iter_mut() {
        session.app.orgs.clone_from(&unread);
        session.app.org = several.then(|| org.to_owned());
    }
}

/// Shows the organisation `name`, or with none the next one, saying so in the app shown when
/// there's none of that name.
pub fn switch(orgs: &mut Orgs<Session>, name: Option<&str>) {
    let next = if let Some(name) = name {
        name.to_owned()
    } else {
        let names: Vec<&str> = orgs.iter().map(|(org, _)| org).collect();
        let at = orgs
            .selected()
            .and_then(|(selected, _)| names.iter().position(|org| *org == selected))
            .unwrap_or(0);
        names[(at + 1) % names.len()].to_owned()
    };
    if !orgs.select(&next) {
        let names: Vec<&str> = orgs.iter().map(|(org, _)| org).collect();
        let said = format!("not signed in to {next}: there's {}", names.join(", "));
        if let Some((_, session)) = orgs.selected_mut() {
            session.app.note(said);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn copies_by_osc_52() {
        assert_eq!(super::osc52("hi ✓"), "\x1b]52;c;aGkg4pyT\x07");
    }
}
