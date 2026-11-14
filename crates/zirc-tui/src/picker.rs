//! The reaction picker: zirc-core's emoji ([`zirc_core::emoji`]), the ones used lately
//! first, in a grid that's searched by typing and moved through with the arrows. It knows
//! which message it reacts to; the app sends the reaction.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use zirc_core::emoji::{self, Newest, Recent, Row};

/// The newest emoji the picker offers, as Unicode's emoji version: terminals' fonts lag.
pub const NEWEST: Newest = (15, 0);

/// How many emoji to a row of the grid.
pub const PER_ROW: usize = 10;

/// What a key did to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// Still picking.
    Open,
    /// Closed without picking.
    Closed,
    /// This emoji was picked.
    Emoji(&'static str),
}

/// A picker open on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    /// The conversation the message is in.
    pub target: String,
    /// The message's id.
    pub id: String,
    /// What's been typed to search.
    pub query: String,
    /// Where the cursor is: a row of [`Picker::rows`] and an emoji along it.
    at: (usize, usize),
}

impl Picker {
    /// A picker for the message `id` in `target`.
    #[must_use]
    pub fn new(target: &str, id: &str) -> Self {
        let mut picker = Self {
            target: target.to_owned(),
            id: id.to_owned(),
            query: String::new(),
            at: (0, 0),
        };
        picker.at = (picker.first_row(&[]), 0);
        picker
    }

    /// The grid's rows, headings among them.
    #[must_use]
    pub fn rows(&self, recent: &Recent) -> Vec<Row> {
        emoji::rows(&self.query, &recent.emoji, NEWEST, PER_ROW)
    }

    /// Where the cursor is, as a row of [`Picker::rows`] and an emoji along it.
    #[must_use]
    pub fn at(&self) -> (usize, usize) {
        self.at
    }

    /// The emoji under the cursor, if there's any to pick.
    #[must_use]
    pub fn chosen(&self, recent: &Recent) -> Option<&'static str> {
        match self.rows(recent).get(self.at.0) {
            Some(Row::Emoji(row)) => row.get(self.at.1).copied(),
            _ => None,
        }
    }

    /// Takes in a key: typing searches, the arrows move, Enter picks and Esc closes.
    pub fn key(&mut self, key: KeyEvent, recent: &Recent) -> Picked {
        match key.code {
            KeyCode::Esc => return Picked::Closed,
            KeyCode::Enter => {
                return self.chosen(recent).map_or(Picked::Open, Picked::Emoji);
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.at = (self.first_row(&recent.emoji), 0);
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.at = (self.first_row(&recent.emoji), 0);
            }
            KeyCode::Left => self.step(-1, recent),
            KeyCode::Right => self.step(1, recent),
            KeyCode::Up => self.line(-1, recent),
            KeyCode::Down => self.line(1, recent),
            _ => {}
        }
        Picked::Open
    }

    /// The first row with emoji in it.
    fn first_row(&self, recent: &[String]) -> usize {
        emoji::rows(&self.query, recent, NEWEST, PER_ROW)
            .iter()
            .position(|row| matches!(row, Row::Emoji(_)))
            .unwrap_or(0)
    }

    /// Moves to the emoji `by` along, on to the next or previous row at the ends.
    fn step(&mut self, by: isize, recent: &Recent) {
        let rows = self.rows(recent);
        let (row, column) = self.at;
        let len = |row: usize| match rows.get(row) {
            Some(Row::Emoji(emoji)) => emoji.len(),
            _ => 0,
        };
        let column = column.cast_signed() + by;
        if (0..len(row).cast_signed()).contains(&column) {
            self.at.1 = column.cast_unsigned();
        } else if let Some(next) = emoji_row(&rows, row, by) {
            self.at = (next, if by < 0 { len(next) - 1 } else { 0 });
        }
    }

    /// Moves to the row of emoji `by` up or down, keeping to the column where it can.
    fn line(&mut self, by: isize, recent: &Recent) {
        let rows = self.rows(recent);
        if let Some(next) = emoji_row(&rows, self.at.0, by)
            && let Row::Emoji(emoji) = &rows[next]
        {
            self.at = (next, self.at.1.min(emoji.len() - 1));
        }
    }
}

/// The next row of emoji from `row` in the direction of `by`, past headings.
fn emoji_row(rows: &[Row], row: usize, by: isize) -> Option<usize> {
    let mut at = row.cast_signed();
    loop {
        at += by.signum();
        let next = usize::try_from(at).ok()?;
        match rows.get(next)? {
            Row::Emoji(_) => return Some(next),
            Row::Heading(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(picker: &mut Picker, recent: &Recent, code: KeyCode) -> Picked {
        picker.key(KeyEvent::from(code), recent)
    }

    #[test]
    fn searches_moves_and_picks() {
        let recent = Recent {
            emoji: vec!["🚀".into(), "👍".into()],
            ..Recent::default()
        };
        let mut picker = Picker::new("#api", "m1");
        // Lately used first.
        assert_eq!(picker.chosen(&recent), Some("🚀"));
        assert_eq!(press(&mut picker, &recent, KeyCode::Right), Picked::Open);
        assert_eq!(picker.chosen(&recent), Some("👍"));
        // Past the end of a row, on to the next one's start, and back.
        press(&mut picker, &recent, KeyCode::Right);
        assert_ne!(picker.chosen(&recent), Some("👍"));
        press(&mut picker, &recent, KeyCode::Left);
        assert_eq!(picker.chosen(&recent), Some("👍"));
        // Down and up keep to the column.
        press(&mut picker, &recent, KeyCode::Down);
        assert_eq!(picker.at().1, 1);
        press(&mut picker, &recent, KeyCode::Up);
        assert_eq!(picker.chosen(&recent), Some("👍"));

        for c in "tada".chars() {
            press(&mut picker, &recent, KeyCode::Char(c));
        }
        assert_eq!(picker.chosen(&recent), Some("🎉"));
        assert_eq!(
            press(&mut picker, &recent, KeyCode::Enter),
            Picked::Emoji("🎉")
        );
        for _ in 0..4 {
            press(&mut picker, &recent, KeyCode::Backspace);
        }
        assert_eq!(picker.chosen(&recent), Some("🚀"));
        // Nothing found: nothing to pick, and Esc closes.
        for c in "zzzzqx".chars() {
            press(&mut picker, &recent, KeyCode::Char(c));
        }
        assert_eq!(picker.chosen(&recent), None);
        assert_eq!(press(&mut picker, &recent, KeyCode::Enter), Picked::Open);
        assert_eq!(press(&mut picker, &recent, KeyCode::Esc), Picked::Closed);
    }
}
