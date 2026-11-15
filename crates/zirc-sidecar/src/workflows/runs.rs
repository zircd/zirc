//! A workflow's ended runs, read back from Ergo's history a page at a time
//! ([`zirc_proto::workflow::RUNS_PATH`]): the run events its nick posted in its channel, the
//! latest first, each run's ending folded together with what it was given and what its steps
//! handed back. What Ergo no longer keeps isn't there, and a part of a step given to several
//! (`master-3.1`) is left to its run.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::params;
use zirc_proto::run::{Change, RunEvent};
use zirc_proto::workflow::{Ended, MAX_ENDED, Runs};

use crate::exporter::{self, Opened, PRIVMSG};

/// How many of the channel's messages one page reads at most: in a busy channel it stops
/// there with what it has, and the next page carries on from where it stopped.
const MOST: usize = 20_000;

/// How many messages are read from the database at a time.
const BATCH: i64 = 500;

/// The page of `nick`'s runs in `channel` that ended before `before` (nanoseconds since the
/// epoch, a page's [`Runs::before`]), or the latest.
///
/// # Errors
///
/// When Ergo's history can't be read.
pub fn page(
    history: &Path,
    channel: &str,
    nick: &str,
    before: Option<i64>,
) -> rusqlite::Result<Runs> {
    let Opened::Open(connection) = exporter::open_history(history)? else {
        return Ok(Runs::default());
    };
    // Ergo keeps a channel's messages under its casefolded name.
    let channel = channel.to_lowercase();
    let mut statement = connection.prepare(
        "SELECT s.nanotime, h.id, h.data FROM sequence s JOIN history h ON h.id = s.history_id \
         WHERE s.target = ?1 AND (s.nanotime, h.id) < (?2, ?3) \
         ORDER BY s.nanotime DESC, h.id DESC LIMIT ?4",
    )?;
    let mut folding = Folding::default();
    // Strictly before `before`: the page's last run ended then.
    let (mut at, mut id) = before.map_or((i64::MAX, i64::MAX), |before| (before, i64::MIN));
    let mut read = 0;
    loop {
        let rows: Vec<(i64, i64, Vec<u8>)> = statement
            .query_map(params![channel, at, id, BATCH], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let last = rows.len() < usize::try_from(BATCH).unwrap_or(usize::MAX);
        for (nanotime, row, data) in rows {
            (at, id, read) = (nanotime, row, read + 1);
            if let Some(event) = run_event(&data, nick) {
                folding.fold(event, nanotime);
            }
            if folding.finished() {
                return Ok(folding.into_page(None));
            }
            if read >= MOST {
                return Ok(folding.into_page(Some(at)));
            }
        }
        if last {
            return Ok(folding.into_page(None));
        }
    }
}

/// The run event in a history item, if it's one `nick` posted.
fn run_event(data: &[u8], nick: &str) -> Option<RunEvent> {
    let said = exporter::said(data)?;
    if said.kind != PRIVMSG || !said.account.eq_ignore_ascii_case(nick) {
        return None;
    }
    let message = zirc_proto::decode(&said.text).ok().flatten()?;
    RunEvent::of(&message.event).filter(|event| !event.run.contains('.'))
}

/// The runs met so far, the latest first, as their events are read back newest first.
#[derive(Default)]
struct Folding {
    runs: Vec<Ended>,
    /// When each run's ending was said, in nanoseconds: where the page after it starts.
    said: Vec<i64>,
    /// How many of them have been read back to their start.
    started: usize,
    /// Whether a run older than a page's was met.
    more: bool,
}

impl Folding {
    fn fold(&mut self, event: RunEvent, nanotime: i64) {
        let RunEvent {
            run, at, change, ..
        } = event;
        let index = self.runs.iter().position(|r| r.run == run);
        match (change, index) {
            (Change::Finished { .. }, None) if self.runs.len() == MAX_ENDED => self.more = true,
            (Change::Finished { outcome, why, step }, None) => {
                self.runs.push(Ended {
                    run,
                    outcome,
                    why,
                    step,
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    started: None,
                    ended: at,
                });
                self.said.push(nanotime);
            }
            // Read newest first: a later step's outputs are met before an earlier's.
            (Change::Done { outputs, .. }, Some(i)) => {
                for (name, value) in outputs {
                    self.runs[i].outputs.entry(name).or_insert(value);
                }
            }
            (Change::Started { inputs, .. }, Some(i)) if self.runs[i].started.is_none() => {
                self.runs[i].inputs = inputs;
                self.runs[i].started = Some(at);
                self.started += 1;
            }
            _ => {}
        }
    }

    /// Whether the page is whole: as many runs as a page has, each read back to its start,
    /// and whether there are more known.
    fn finished(&self) -> bool {
        self.runs.len() == MAX_ENDED && self.started == MAX_ENDED && self.more
    }

    /// The page, which a read that stopped at `stopped` carries on from.
    fn into_page(self, stopped: Option<i64>) -> Runs {
        let before = if self.more {
            self.said.last().copied()
        } else {
            stopped
        };
        Runs {
            runs: self.runs,
            before: before.map(|at| at.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {

    use zirc_proto::run::Outcome;

    use super::*;
    use crate::trim::testing::{add, history};

    const SECOND: i64 = 1_000_000_000;

    fn said(path: &Path, nick: &str, at: u64, run: &str, change: Change) {
        let event = RunEvent {
            run: run.to_owned(),
            at,
            change,
            card: serde_json::from_value(serde_json::json!({"id": run, "title": run})).unwrap(),
        };
        let message = zirc_proto::Message::new("flaky", event.event(format!("`{run}`")));
        let text = zirc_proto::encode(&message).unwrap();
        let nanotime = i64::try_from(at).unwrap() * SECOND;
        add(path, &format!("{run}-{at}"), "#ops", nanotime, nick, &text);
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn started(test: &str) -> Change {
        Change::Started {
            by: "watcher".to_owned(),
            inputs: map(&[("test", test)]),
            version: "v1".to_owned(),
        }
    }

    #[test]
    fn a_run_is_read_back_with_its_inputs_outputs_and_the_step_it_failed_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        said(&path, "flake_bot", 100, "flaky-1", started("//a:test"));
        let done = Change::Done {
            step: "fix".to_owned(),
            by: "cc-1".to_owned(),
            outputs: map(&[("cause", "a race")]),
        };
        said(&path, "flake_bot", 160, "flaky-1", done);
        let finished = Change::Finished {
            outcome: Outcome::Done,
            why: None,
            step: None,
        };
        said(&path, "flake_bot", 170, "flaky-1", finished);
        said(&path, "flake_bot", 200, "flaky-2", started("//b:test"));
        // A part, and someone else posting a run's event, are left out.
        said(&path, "flake_bot", 210, "flaky-2.1", started("//c:test"));
        said(&path, "mallory", 220, "flaky-9", started("//d:test"));
        let failed = Change::Finished {
            outcome: Outcome::Failed,
            why: Some("not reproducible".to_owned()),
            step: Some("fix".to_owned()),
        };
        said(&path, "flake_bot", 300, "flaky-2", failed);

        let page = page(&path, "#OPS", "Flake_Bot", None).unwrap();

        let expected = vec![
            Ended {
                run: "flaky-2".to_owned(),
                outcome: Outcome::Failed,
                why: Some("not reproducible".to_owned()),
                step: Some("fix".to_owned()),
                inputs: map(&[("test", "//b:test")]),
                outputs: BTreeMap::new(),
                started: Some(200),
                ended: 300,
            },
            Ended {
                run: "flaky-1".to_owned(),
                outcome: Outcome::Done,
                why: None,
                step: None,
                inputs: map(&[("test", "//a:test")]),
                outputs: map(&[("cause", "a race")]),
                started: Some(100),
                ended: 170,
            },
        ];
        assert_eq!(
            page,
            Runs {
                runs: expected,
                before: None
            }
        );
    }

    #[test]
    fn runs_are_paged_from_where_the_last_page_ended() {
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let finished = || Change::Finished {
            outcome: Outcome::Done,
            why: None,
            step: None,
        };
        for n in 0..MAX_ENDED as u64 + 3 {
            let run = format!("flaky-{n}");
            said(&path, "flake_bot", 1000 + n * 10, &run, started("//a:test"));
            said(&path, "flake_bot", 1005 + n * 10, &run, finished());
        }

        let first = page(&path, "#ops", "flake_bot", None).unwrap();
        let before = first.before.as_deref().map(|b| b.parse().unwrap());
        let second = page(&path, "#ops", "flake_bot", before).unwrap();

        let runs = |page: &Runs| page.runs.iter().map(|r| r.run.clone()).collect::<Vec<_>>();
        assert_eq!(first.runs.len(), MAX_ENDED);
        assert_eq!(first.runs[0].run, "flaky-22");
        assert!(first.runs.iter().all(|r| r.started.is_some()));
        assert_eq!(runs(&second), ["flaky-2", "flaky-1", "flaky-0"]);
        assert_eq!(second.before, None);
    }
}
