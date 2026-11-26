//! Files waiting above the message field to go with the next message (the user's choices,
//! 23 Nov 2026): a file pasted, picked or dropped waits here, shown as a thumbnail with an
//! ✕, and nothing is uploaded until Send. Then each uploads, busy on its thumbnail, and
//! once all have, they go with the words as one message ([`Chat::say_attached`]). One that
//! fails stays, saying why, and Send tries it again; nothing half-sent is ever said.
//!
//! What waits, how many may, and how each stands is decided here once. An app keeps the
//! bytes by their [`Attached::id`], draws [`Chat::attached`], and uploads what
//! [`Chat::upload_attached`] names; it decides nothing.

use serde::Serialize;
use ts_rs::TS;
use zirc_proto::File;
use zirc_proto::attached::MAX_FILES;

use super::lines::FileShape;
use super::{Chat, Here, Ran, file, fold};
use crate::command::Command;

/// A file waiting above the message field, as every app draws it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Attached {
    /// What the app keeps its bytes by, and names it by to take it off or say how its
    /// upload went: its own in the conversation, never reused there.
    pub id: u32,
    /// Its name.
    pub name: String,
    /// Its size, as every app says one, such as `2.3 MB`.
    pub size_text: String,
    /// How it will be drawn once sent, by what its name says it is: an image shows as a
    /// thumbnail of its bytes, anything else as its name.
    pub shape: FileShape,
    /// Whether it's uploading: a spinner on it, and no ✕.
    pub busy: bool,
    /// Why its upload failed, to say on it: Send tries it again.
    pub failed: Option<String>,
}

/// How a waiting file stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Standing {
    /// Not sent yet.
    Waiting,
    /// Uploading, for the message being sent.
    Uploading,
    /// Uploaded: what the message will carry.
    Uploaded(File),
    /// Its upload failed, and why.
    Failed(String),
}

/// A file waiting in a conversation.
#[derive(Debug, Clone)]
pub(super) struct Waiting {
    id: u32,
    name: String,
    size: u64,
    standing: Standing,
}

/// The files waiting in each conversation, by its folded name, the message each is being
/// sent with, and the next id to give.
#[derive(Debug, Default)]
pub(super) struct Attaching {
    waiting: std::collections::HashMap<String, Vec<Waiting>>,
    sending: std::collections::HashMap<String, Message>,
    next: u32,
}

/// The message a conversation's files are uploading for: what was typed when Send was
/// pressed, which is what goes once they're in.
#[derive(Debug, Clone)]
struct Message {
    words: String,
    replying: Option<String>,
    newest: crate::emoji::Newest,
}

/// What Send comes to in a conversation that may have files waiting ([`Chat::send_attached`]).
#[derive(Debug)]
pub enum Sending {
    /// No file waits: send what's typed as ever ([`Chat::run`]).
    Nothing,
    /// Upload these, each busy from here, and say how each went ([`Chat::attached_done`]):
    /// the message goes once all are in. Until then the field keeps its words and waits.
    Upload(Vec<u32>),
    /// The files were uploaded before, by a send that couldn't be said: this is it said.
    Said(Ran),
    /// It can't go as it is, and why.
    Refused(String),
}

/// What Send says of a command typed with files waiting.
pub const COMMANDS_TAKE_NO_FILES: &str = "A command takes no files: take them off first.";

/// What attaching a file says while the conversation's message is being sent.
pub const SENDING: &str = "Wait for the message being sent.";

/// What a waiting file's ✕ is called, for a screen reader and a pointer over it:
/// `Remove shot.png`.
#[must_use]
pub fn detach_label(name: &str) -> String {
    format!("Remove {name}")
}

/// What attaching a file past [`MAX_FILES`] says.
#[must_use]
pub fn too_many_label() -> String {
    format!("A message takes {MAX_FILES} files at most.")
}

impl Chat {
    /// Puts a file called `name`, of `size` bytes, above the field of the conversation
    /// `here`, to go with its next message: its id, which the app keeps its bytes by.
    ///
    /// # Errors
    ///
    /// What to say when it can't wait there: the conversation already has
    /// [`MAX_FILES`], or is sending.
    pub fn attach(&mut self, here: &str, name: &str, size: u64) -> Result<u32, String> {
        if self.sending_attached(here) {
            return Err(SENDING.to_owned());
        }
        let attaching = &mut self.attaching;
        let waiting = attaching.waiting.entry(fold(here)).or_default();
        if waiting.len() >= MAX_FILES {
            return Err(too_many_label());
        }
        attaching.next += 1;
        waiting.push(Waiting {
            id: attaching.next,
            name: name.to_owned(),
            size,
            standing: Standing::Waiting,
        });
        Ok(attaching.next)
    }

    /// Takes the file `id` off the field of `here`, as its ✕ does; not one uploading.
    pub fn detach(&mut self, here: &str, id: u32) {
        if let Some(waiting) = self.attaching.waiting.get_mut(&fold(here)) {
            waiting.retain(|file| file.id != id || file.standing == Standing::Uploading);
        }
    }

    /// The files waiting above the field of `here`, in the order they were attached.
    #[must_use]
    pub fn attached(&self, here: &str) -> Vec<Attached> {
        let waiting = self.attaching.waiting.get(&fold(here));
        waiting
            .into_iter()
            .flatten()
            .map(|waiting| Attached {
                id: waiting.id,
                name: waiting.name.clone(),
                size_text: file::size_text(waiting.size),
                shape: file::shape_of(zirc_proto::uploads::guess_mime(&waiting.name)).into(),
                busy: waiting.standing == Standing::Uploading,
                failed: match &waiting.standing {
                    Standing::Failed(why) => Some(why.clone()),
                    _ => None,
                },
            })
            .collect()
    }

    /// Whether the message of `here` is being sent, its files uploading: the field and its
    /// Send wait, as nothing typed meanwhile would go with it.
    #[must_use]
    pub fn sending_attached(&self, here: &str) -> bool {
        let waiting = self.attaching.waiting.get(&fold(here));
        waiting
            .into_iter()
            .flatten()
            .any(|file| file.standing == Standing::Uploading)
    }

    /// Send, with files waiting in `here`: the ids of those to upload now, each busy from
    /// here, which are those not uploaded yet, one that failed among them. None when none
    /// waits, or all are uploaded already: see [`Chat::attached_files`].
    pub fn upload_attached(&mut self, here: &str) -> Vec<u32> {
        let Some(waiting) = self.attaching.waiting.get_mut(&fold(here)) else {
            return Vec::new();
        };
        waiting
            .iter_mut()
            .filter(|file| !matches!(file.standing, Standing::Uploaded(_)))
            .map(|file| {
                file.standing = Standing::Uploading;
                file.id
            })
            .collect()
    }

    /// The file `id` of `here` is uploaded, as `file`.
    pub fn attached_uploaded(&mut self, here: &str, id: u32, file: File) {
        self.stands(here, id, Standing::Uploaded(file));
    }

    /// The file `id` of `here` couldn't be uploaded, and `why`: it stays, saying so.
    pub fn attached_failed(&mut self, here: &str, id: u32, why: &str) {
        self.stands(here, id, Standing::Failed(why.to_owned()));
    }

    fn stands(&mut self, here: &str, id: u32, standing: Standing) {
        let waiting = self.attaching.waiting.get_mut(&fold(here));
        if let Some(file) = waiting
            .into_iter()
            .flatten()
            .find(|file| file.id == id && file.standing == Standing::Uploading)
        {
            file.standing = standing;
        }
    }

    /// The files of `here` to say with its words, once every one waiting is uploaded:
    /// `None` while one isn't, or none waits.
    #[must_use]
    pub fn attached_files(&self, here: &str) -> Option<Vec<File>> {
        let waiting = self.attaching.waiting.get(&fold(here))?;
        let files: Option<Vec<File>> = waiting
            .iter()
            .map(|file| match &file.standing {
                Standing::Uploaded(file) => Some(file.clone()),
                _ => None,
            })
            .collect();
        files.filter(|files| !files.is_empty())
    }

    /// Nothing waits above the field of `here` any more: its message was said.
    pub fn clear_attached(&mut self, here: &str) {
        self.attaching.waiting.remove(&fold(here));
        self.attaching.sending.remove(&fold(here));
    }

    /// Send, pressed in `here` with `text` typed, in every app: with files waiting there,
    /// what to upload first, the words kept to go with them. A message being corrected
    /// takes none: it goes as ever, and the files wait on.
    pub fn send_attached(&mut self, here: &Here<'_>, text: &str) -> Sending {
        let name = here.name;
        if here.editing.is_some() || self.attached(name).is_empty() {
            return Sending::Nothing;
        }
        let words = match crate::command::parse(text.trim()) {
            Ok(Command::Say(words)) => words,
            _ if text.trim().is_empty() => String::new(),
            _ => return Sending::Refused(COMMANDS_TAKE_NO_FILES.to_owned()),
        };
        let message = Message {
            words,
            replying: here.replying.map(str::to_owned),
            newest: here.newest,
        };
        self.attaching.sending.insert(fold(name), message);
        let upload = self.upload_attached(name);
        if upload.is_empty() {
            self.say_waiting(name)
                .map_or(Sending::Nothing, Sending::Said)
        } else {
            Sending::Upload(upload)
        }
    }

    /// How the upload of the file `id` of `here` went: `file`, or why not. Once every one is
    /// in, the message is said, and this is what to send ([`Ran::Send`] or [`Ran::Replied`];
    /// anything else, such as a note, and the files wait on, uploaded). `None` while one
    /// still uploads, or one failed: it stays, saying why, and Send tries it again.
    pub fn attached_done(
        &mut self,
        here: &str,
        id: u32,
        outcome: Result<File, String>,
    ) -> Option<Ran> {
        match outcome {
            Ok(file) => self.attached_uploaded(here, id, file),
            Err(why) => self.attached_failed(here, id, &why),
        }
        self.say_waiting(here)
    }

    /// Says the message `here`'s files were uploading for, once all are in: with words, or
    /// several files, as one message ([`Chat::say_attached`]); a file alone as ever
    /// ([`Chat::share`]), which every zirc draws.
    fn say_waiting(&mut self, here: &str) -> Option<Ran> {
        let files = self.attached_files(here)?;
        let message = self.attaching.sending.get(&fold(here))?.clone();
        let ran = match files.as_slice() {
            [file] if message.words.is_empty() => {
                let me = self.me.clone();
                Ran::Send(self.share(here, &me, file.clone()))
            }
            files => self.say_attached(
                &Here {
                    replying: message.replying.as_deref(),
                    ..Here::new(here, message.newest)
                },
                &message.words,
                files,
            ),
        };
        if matches!(ran, Ran::Send(_) | Ran::Replied(_)) {
            self.clear_attached(here);
        }
        Some(ran)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Options;

    fn chat() -> Chat {
        Chat::new("me", "irc.zirc.test", Vec::new(), Options::default())
    }

    fn uploaded(name: &str) -> File {
        File {
            url: format!("https://zirc.dev/m/zirc/{name}"),
            name: name.to_owned(),
            mime: "image/png".into(),
            size: 2048,
            width: None,
            height: None,
            duration_ms: None,
        }
    }

    #[test]
    fn files_wait_by_conversation_until_send_uploads_them() {
        let mut chat = chat();
        let shot = chat.attach("#api", "shot.png", 2_345_678).unwrap();
        let notes = chat.attach("#API", "notes.txt", 12).unwrap();
        assert!(chat.attached("#web").is_empty());
        let waiting = chat.attached("#api");
        assert_eq!(
            (
                waiting[0].name.as_str(),
                waiting[0].size_text.as_str(),
                waiting[0].shape
            ),
            ("shot.png", "2.3 MB", FileShape::Image)
        );
        assert_eq!(
            (waiting[1].shape, waiting[1].busy),
            (FileShape::Text, false)
        );
        assert_eq!(chat.attached_files("#api"), None);

        assert_eq!(chat.upload_attached("#api"), [shot, notes]);
        assert!(chat.sending_attached("#api"));
        // Not while it uploads: its ✕ is gone, and nothing more is taken.
        chat.detach("#api", shot);
        assert!(chat.attach("#api", "late.png", 1).is_err());
        assert_eq!(chat.attached("#api").len(), 2);

        chat.attached_uploaded("#api", shot, uploaded("shot.png"));
        chat.attached_failed("#api", notes, "too large: files are 25 MB at most");
        assert!(!chat.sending_attached("#api"));
        assert_eq!(chat.attached_files("#api"), None);
        let failed = &chat.attached("#api")[1];
        assert_eq!(
            failed.failed.as_deref(),
            Some("too large: files are 25 MB at most")
        );
    }

    #[test]
    fn send_again_uploads_only_what_failed_and_then_all_go() {
        let mut chat = chat();
        let shot = chat.attach("#api", "shot.png", 1).unwrap();
        let notes = chat.attach("#api", "notes.txt", 1).unwrap();
        let _ = chat.upload_attached("#api");
        chat.attached_uploaded("#api", shot, uploaded("shot.png"));
        chat.attached_failed("#api", notes, "offline");

        assert_eq!(chat.upload_attached("#api"), [notes]);
        chat.attached_uploaded("#api", notes, uploaded("notes.txt"));
        let files = chat.attached_files("#api").unwrap();
        assert_eq!(files, [uploaded("shot.png"), uploaded("notes.txt")]);
        chat.clear_attached("#api");
        assert!(chat.attached("#api").is_empty());
    }

    /// Send with files waiting: they upload, and once all are in the words go with them as
    /// one message; a file alone goes as a shared file ever did; a command is refused.
    #[test]
    fn send_uploads_what_waits_and_then_says_the_words_with_the_files() {
        let mut chat = chat();
        chat.connected(crate::chat::Link {
            nick: "me".into(),
            chathistory: true,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
            metadata: false,
        });
        let _ = chat.receive(":me!u@h JOIN #api".parse().unwrap());
        let here = Here::new("#api", (15, 0));
        assert!(matches!(
            chat.send_attached(&here, "no files"),
            Sending::Nothing
        ));

        let shot = chat.attach("#api", "shot.png", 1).unwrap();
        let Sending::Refused(why) = chat.send_attached(&here, "/me waves") else {
            panic!("a command takes no files");
        };
        assert_eq!(why, COMMANDS_TAKE_NO_FILES);
        let Sending::Upload(ids) = chat.send_attached(&here, "before and after") else {
            panic!("its file uploads first");
        };
        assert_eq!(ids, [shot]);
        let Some(Ran::Send(lines)) = chat.attached_done("#api", shot, Ok(uploaded("shot.png")))
        else {
            panic!("all are in: it's said");
        };
        assert_eq!(
            lines[0].params[1],
            "before and after https://zirc.dev/m/zirc/shot.png"
        );
        assert!(lines[0].tag(zirc_proto::attached::TAG).is_some());
        assert!(chat.attached("#api").is_empty());

        // A file with no words beside it: a shared file, as ever.
        let alone = chat.attach("#api", "alone.png", 1).unwrap();
        assert!(matches!(chat.send_attached(&here, ""), Sending::Upload(_)));
        let Some(Ran::Send(lines)) = chat.attached_done("#api", alone, Ok(uploaded("alone.png")))
        else {
            panic!("it's shared");
        };
        assert!(lines[0].tag(zirc_proto::attached::TAG).is_none());
        assert!(lines[0].params[1].starts_with("\u{1}ACTION ZIRC "));
    }

    #[test]
    fn a_message_takes_only_so_many_and_one_can_be_taken_off() {
        let mut chat = chat();
        let ids: Vec<u32> = (0..MAX_FILES)
            .map(|n| chat.attach("#api", &format!("{n}.png"), 1).unwrap())
            .collect();
        assert_eq!(chat.attach("#api", "more.png", 1), Err(too_many_label()));
        chat.detach("#api", ids[0]);
        assert_eq!(chat.attached("#api").len(), MAX_FILES - 1);
        // A new one's id is its own, never one taken off.
        assert!(!ids.contains(&chat.attach("#api", "more.png", 1).unwrap()));
    }
}
