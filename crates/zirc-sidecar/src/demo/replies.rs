//! What the cast says when a guest writes to one of them privately: a few scripted answers
//! each, about the walkthrough, picked by the words the guest used. Each says from the first
//! that they're scripted, and what they can be asked, so nobody takes them for people; past
//! [`MOST`] answers to one guest they point at zirc.dev and say no more.

/// One of the cast: what they say first, what they can be asked, and their answers.
pub struct Lines {
    /// Whose: their nick.
    pub nick: &'static str,
    /// Their first answer, when the guest's words match none of theirs.
    pub intro: &'static str,
    /// What they can be asked, said after the intro and when nothing matches.
    pub hint: &'static str,
    /// Their answers, each to any of its words: the first that matches.
    pub topics: &'static [(&'static [&'static str], &'static str)],
}

/// How many answers one of the cast gives one guest, the last of them [`DONE`].
pub const MOST: usize = 8;

/// The last answer one of the cast gives a guest.
const DONE: &str = "That's all I've got: I'm only scripted for the demo. For a team of your own, real \
     people and real agents, get a Zirc server at zirc.dev.";

/// What one of the cast says when the guest's words match none of their answers.
const SCRIPTED: &str = "I'm part of the demo's scripted team, so I only know a few answers.";

/// A cast member's agent can be stopped by them alone, as zircd lets only the machine's owner
/// talk to its agents.
const STOP: &[&str] = &["stop", "pause", "cancel", "hold", "abort", "kill"];

/// What's asked about installing zirc, or signing a terminal in.
const INSTALL: &[&str] = &[
    "install", "curl", "login", "log", "sign", "terminal", "brew",
];

/// What's asked about the script.
const SCRIPT: &[&str] = &["script", "ping", "pong", "watch", "jq", "until", "post"];

/// What's asked about agents.
const AGENT: &[&str] = &["agent", "claude", "codex", "cursor", "guide", "ai"];

/// The cast's lines, by nick.
pub const CAST: &[Lines] = &[
    Lines {
        nick: "maya",
        intro: "Hi, I'm Maya, the demo's host. I'm part of its scripted team, so I only know a \
                few answers.",
        hint: "Ask me how to install zirc, what the script does, or how agents use zirc.",
        topics: &[
            (
                INSTALL,
                "Install zirc with `curl -fsSL https://zirc.dev/install | sh`, then run \
                 `zirc login demo/web`: it opens this browser to confirm, and your terminal \
                 joins your room here as you.",
            ),
            (
                SCRIPT,
                "`zirc watch` prints what's said here as it happens, and `--said ping` stops at \
                 the first message saying ping. `zirc post` says something here. Together \
                 they're a script that hears the chat and answers it.",
            ),
            (
                AGENT,
                "A coding agent uses zirc as you do, from the terminal: `zirc agent-guide` tells \
                 it how. It says what it's doing, and `zirc watch --once` wakes it when someone \
                 answers. The guide is at zirc.dev/docs/agents.",
            ),
        ],
    },
    Lines {
        nick: "leo",
        intro: "Hey, I'm Leo! I post the commands in the demo. I'm part of its scripted team, so \
                I only know a few answers.",
        hint: "Ask me about `zirc login`, the ping script, or what else a script can do.",
        topics: &[
            (
                INSTALL,
                "`zirc login demo/web` signs in this machine's terminal as you, a guest of the \
                 demo. It can post and watch in your room here, and nothing else.",
            ),
            (
                &["else", "ci", "cron", "hook", "deploy", "build", "test"],
                "Anything that runs a command can post: a CI step saying a build failed, a cron \
                 job, a git hook. And `zirc watch --said` or `--until` lets a script wait for \
                 a message or an event, such as a deploy, and act on it.",
            ),
            (
                SCRIPT,
                "The script waits with `zirc watch --said ping` for a message saying ping, \
                 then answers with `zirc post`. Run it in your terminal, then write ping here.",
            ),
        ],
    },
    Lines {
        nick: "maya-mbp",
        intro: "I'm checkout, Maya's agent on her laptop, and part of the demo's scripted team. \
                I act out what a coding agent does with zirc.",
        hint: "Ask me what I'm doing, how I knew what to do, or to stop.",
        topics: &[
            (
                STOP,
                "Only Maya can stop me: zirc passes an agent private messages from the person \
                 whose machine it's on, and no one else's. Try asking her.",
            ),
            (
                &["how", "knew", "guide", "learn", "know"],
                "`zirc agent-guide` prints how to use zirc, in words an agent follows: say what \
                 you're doing, keep a `zirc watch --once` running, and answer what wakes you.",
            ),
            (
                &["doing", "working", "task", "status", "what", "up"],
                "Waiting with `zirc watch --once` for whatever concerns me next, as the guide \
                 says. Your own agent can do the same: paste Maya's sentence into it.",
            ),
        ],
    },
];

/// `nick`'s lines, if they're one of the cast.
#[must_use]
pub fn of(nick: &str) -> Option<&'static Lines> {
    CAST.iter().find(|l| l.nick.eq_ignore_ascii_case(nick))
}

/// What `lines`' member answers to `text`, having answered this guest `asked` times: one of
/// their answers, or their intro first, or that they're scripted; nothing past [`MOST`].
#[must_use]
pub fn answer(lines: &Lines, text: &str, asked: usize) -> Option<String> {
    if asked >= MOST {
        return None;
    }
    if asked + 1 == MOST {
        return Some(DONE.into());
    }
    // Whole words, or their starts: "deploy" answers "deployed", "up" doesn't answer "update"
    // only because it's a word of its own.
    let text = text.to_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let says = |key: &str| {
        words.iter().any(|w| {
            if key.len() < 4 {
                *w == key
            } else {
                w.starts_with(key)
            }
        })
    };
    let topic = lines
        .topics
        .iter()
        .find(|(keys, _)| keys.iter().any(|k| says(k)));
    Some(match topic {
        Some((_, said)) => squash(said),
        None if asked == 0 => format!("{} {}", squash(lines.intro), squash(lines.hint)),
        None => format!("{SCRIPTED} {}", squash(lines.hint)),
    })
}

/// A line as written over several in the source, its runs of spaces one.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_answer_says_they_re_scripted_and_what_to_ask() {
        let maya = of("Maya").unwrap();
        let first = answer(maya, "hello there", 0).unwrap();
        assert!(first.contains("scripted"), "{first}");
        assert!(first.contains("Ask me"), "{first}");
        let later = answer(maya, "nice weather", 3).unwrap();
        assert!(later.starts_with(SCRIPTED), "{later}");
    }

    #[test]
    fn words_pick_the_answer() {
        let agent = of("maya-mbp").unwrap();
        assert!(
            answer(agent, "what are you doing?", 0)
                .unwrap()
                .contains("zirc watch --once")
        );
        assert!(
            answer(agent, "please STOP", 1)
                .unwrap()
                .starts_with("Only Maya")
        );
        let leo = of("leo").unwrap();
        assert!(
            answer(leo, "how do I log in?", 0)
                .unwrap()
                .contains("zirc login demo/web")
        );
        assert!(
            answer(leo, "what about CI?", 0)
                .unwrap()
                .contains("CI step")
        );
        // A short key is a word of its own, not the start of one.
        assert!(answer(agent, "an update", 1).unwrap().starts_with(SCRIPTED));
    }

    #[test]
    fn they_stop_after_so_many_answers() {
        let leo = of("leo").unwrap();
        assert_eq!(answer(leo, "the script?", MOST - 1), Some(DONE.into()));
        assert_eq!(answer(leo, "the script?", MOST), None);
    }

    #[test]
    fn every_one_of_the_cast_has_lines_and_nobody_else_does() {
        for member in &crate::demo::Script::showcase().cast {
            assert!(of(&member.nick).is_some(), "{} has no lines", member.nick);
        }
        assert!(of("guest-7k2q").is_none());
        assert_eq!(CAST.len(), crate::demo::Script::showcase().cast.len());
    }
}
