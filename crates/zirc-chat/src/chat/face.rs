//! Faces, as every app draws them beside a sidebar row, a message's head or a quote: a
//! person's photo or initials on their colour, an agent's tool or picture on its colour with
//! its owner's face small, or one of zirc's apps by its glyph. Everything a face needs is
//! here, so no app works out initials, colours or where a photo loads from.

use serde::Serialize;
use ts_rs::TS;

use super::conversation::Entry;
use super::{Avatar, Chat, nick_degrees};

/// What an avatar shows, ready to draw: [`Avatar`] with its initials, colours and photos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(rename_all = "camelCase", tag = "kind")]
pub enum Face {
    /// A person: their photo, round, or their initials on their colour.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Person {
        /// Their nick, which a click on the face opens the card of.
        nick: String,
        /// Up to two letters, of the name they show as: `HS` for Henrik Sandberg.
        initials: String,
        /// Their colour, a hue in degrees from 0 up to 359, from their nick so it stays as
        /// names change ([`nick_degrees`]). Phones divide it by 360.
        hue: u16,
        /// Their photo's address, drawn in place of the initials: `None` for initials.
        photo: Option<String>,
    },
    /// An agent: a round icon of its tool, or the picture it chose, on its colour, with its
    /// owner's face small at the bottom right.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Agent {
        /// The tool it runs in, such as `claude-code` ([`zirc_proto::CLAUDE_CODE`]), for its
        /// icon ([`super::tool_mark`]); `None` for a generic one.
        tool: Option<String>,
        /// Its colour, a hue in degrees, the same every time for the same session.
        hue: u16,
        /// Whose agent it is, by nick; empty for one that's nobody's, a role, which every app
        /// draws without its owner's small face.
        owner: String,
        /// The owner's initials, for the small face, as a person's.
        owner_initials: String,
        /// The owner's colour, a hue in degrees.
        owner_hue: u16,
        /// The owner's photo's address; `None` for their initials.
        owner_photo: Option<String>,
        /// The session's id, `cc-598aa1`; `None` for the machine itself and for a role.
        session: Option<String>,
        /// The bot it's drawn as, one of [`zirc_proto::AGENT_AVATARS`] such as `reviewer`,
        /// which the apps bundle and draw white on its colour: the role it says, else the one
        /// its name tells, else the plain bot. `None` for the machine itself, drawn with its
        /// tool's icon.
        picture: Option<String>,
        /// The hue its square is drawn on, decided here for every app: `hue` for a session
        /// or a role; `None` for the machine itself, on the muted ground.
        ground: Option<u16>,
    },
    /// One of zirc's apps, CI or Alerts, or a machine's thread: a square with a glyph, never
    /// initials.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    App {
        /// What it draws, at most two letters: `CI`.
        glyph: String,
    },
}

/// Who a line is from, as its head shows them: [`super::Speaker`] with its face ready to
/// draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineSpeaker {
    /// The avatar.
    pub face: Face,
    /// The name line, in bold: a person's name, an agent session's name, an app's name.
    pub name: String,
    /// What follows the name, muted: `mattias's agent` for an agent session.
    pub detail: Option<String>,
    /// What a screen reader says of them: `zirc-chat, agent of mattias`.
    pub label: String,
    /// For an agent session, its id, `cc-598aa1`, whose card a click on them opens.
    pub session: Option<String>,
    /// Whether it's one of zirc's apps, CI or Alerts, which [`super::APP_TAG`] follows.
    pub app: bool,
    /// The tag after its name, decided here for every app, which draws it as it's given:
    /// [`super::APP_TAG`] for one of zirc's apps, [`super::ROLE_TAG`] for a role; none for a
    /// person or an agent session.
    pub tag: Option<String>,
}

/// Where pictures load from: the web app and the organisation signed in to
/// ([`Chat::set_picture_site`]); and shared files, when not from the web app
/// ([`Chat::set_files_site`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct PictureSite {
    /// The web app, such as `https://zirc.dev`.
    pub(crate) web: Option<String>,
    /// The organisation's slug, such as `zirc`.
    pub(crate) org: Option<String>,
    /// Where the organisation's files are, when it isn't the web app: a self-hosted
    /// organisation's own server, such as `https://irc.zirc.com`.
    pub(crate) files: Option<String>,
}

impl Chat {
    /// Sets where pictures and files load from: the web app `web`, such as
    /// `https://zirc.dev`, and the organisation `org` signed in to, such as `zirc`. Until
    /// both are set, faces show initials; until `web` is, shared files show as notes, as
    /// nothing may be loaded from an address no one checked.
    pub fn set_picture_site(&mut self, web: Option<String>, org: Option<String>) {
        self.picture_site.web = web;
        self.picture_site.org = org;
    }

    /// Sets where the organisation's shared files are when it isn't the web app: a
    /// self-hosted organisation's own server, such as `https://irc.zirc.com`
    /// (`zirc_core::signin::SignIn::media`). Its files are drawn, and the web app's no
    /// longer are; people's pictures still load from the web app.
    pub fn set_files_site(&mut self, files: Option<String>) {
        self.picture_site.files = files;
    }

    /// The web app pictures load from, as [`Chat::set_picture_site`] set it.
    #[must_use]
    pub fn picture_web(&self) -> Option<&str> {
        self.picture_site.web.as_deref()
    }

    /// Where shared files are drawn from: the organisation's own server
    /// ([`Chat::set_files_site`]), or else the web app.
    #[must_use]
    pub fn files_web(&self) -> Option<&str> {
        self.picture_site
            .files
            .as_deref()
            .or(self.picture_site.web.as_deref())
    }

    /// `nick`'s photo's address, as every app loads it: the web app's member page for it,
    /// `<web>/m/<org>/@<nick>?v=<version>`, which the person's sign-in lets them see, with
    /// the version zirc-chat knows so a new one isn't cached as the old. Never the address
    /// the profile gives, which wants an app's credentials. `None` for initials, and while
    /// [`Chat::set_picture_site`] hasn't said where.
    #[must_use]
    pub fn photo(&self, nick: &str) -> Option<String> {
        let known = self.avatar_url(nick)?;
        let version = known
            .split_once('?')
            .and_then(|(_, query)| query.split('&').find_map(|pair| pair.strip_prefix("v=")))
            .unwrap_or_default();
        let web = self.picture_site.web.as_deref()?.trim_end_matches('/');
        let org = self.picture_site.org.as_deref()?;
        Some(format!("{web}/m/{org}/@{nick}?v={version}"))
    }

    /// `avatar` as every app draws it.
    #[must_use]
    pub fn face(&self, avatar: &Avatar) -> Face {
        match avatar {
            Avatar::Person { nick } if self.is_role(nick) => self.role_face(nick),
            Avatar::Person { nick } => Face::Person {
                nick: nick.clone(),
                initials: self.avatar_initials(nick),
                hue: nick_degrees(nick),
                photo: self.photo(nick),
            },
            Avatar::Agent {
                tool,
                hue,
                owner,
                session,
                picture,
            } => Face::Agent {
                tool: tool.clone(),
                hue: *hue,
                owner: owner.clone(),
                owner_initials: self.avatar_initials(owner),
                owner_hue: nick_degrees(owner),
                owner_photo: self.photo(owner),
                ground: session.is_some().then_some(*hue),
                session: session.clone(),
                picture: session
                    .as_deref()
                    .map(|session| self.agent_picture(session, picture.as_deref())),
            },
            Avatar::App { glyph, .. } => Face::App {
                glyph: glyph.clone(),
            },
        }
    }

    /// The bot the agent session `session` is drawn as ([`zirc_proto::agent_picture`]): the
    /// role it says, in `chosen` or its latest message, else the one its name tells.
    fn agent_picture(&self, session: &str, chosen: Option<&str>) -> String {
        let chosen = chosen.or_else(|| self.session_pictures.get(session).map(String::as_str));
        let name = self.session_names.get(session).map(String::as_str);
        zirc_proto::agent_picture(chosen, name).to_owned()
    }

    /// Who `entry` is from, as a line's head shows them ([`Chat::speaker`]): with the
    /// repository its line is about after the detail, `mattias's agent · api`, once `#agents`
    /// has heard more than one repository (the user's choice, 20 Nov 2026), so a team with
    /// one sees nothing new.
    #[must_use]
    pub fn line_speaker(&self, entry: &Entry) -> LineSpeaker {
        let speaker = self.speaker(entry);
        let session = match &speaker.avatar {
            Avatar::Agent { session, .. } => session.clone(),
            _ => None,
        };
        let several = self.presence.repositories(zirc_proto::AGENTS_CHANNEL) > 1;
        let (detail, label) = match entry.repo().filter(|_| several) {
            Some(repo) => (
                Some(match speaker.detail {
                    Some(detail) => format!("{detail} · {repo}"),
                    None => repo.to_owned(),
                }),
                format!("{}, in {repo}", speaker.label),
            ),
            None => (speaker.detail, speaker.label),
        };
        LineSpeaker {
            face: self.face(&speaker.avatar),
            name: speaker.name,
            detail,
            label,
            session,
            app: speaker.is_app,
            tag: if speaker.is_app {
                Some(super::APP_TAG.to_owned())
            } else if self.is_role(&entry.from) {
                Some(super::ROLE_TAG.to_owned())
            } else {
                None
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{AGENTS_GLYPH, Options};

    /// A chat that knows Ada's profile, `Ada Lovelace` with a photo, and Bob's, without one.
    fn chat() -> Chat {
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        let profiles: zirc_proto::profile::Profiles = serde_json::from_str(
            r#"{"ada": {"name": "Ada Lovelace", "avatar": "7"}, "bob": {"name": "Bob", "avatar": null}}"#,
        )
        .unwrap();
        chat.take_profiles(&profiles, "https://zirc.dev/");
        chat
    }

    fn person(nick: &str) -> Avatar {
        Avatar::Person { nick: nick.into() }
    }

    #[test]
    fn a_persons_initials_are_of_the_name_they_show_as() {
        let chat = chat();
        let Face::Person {
            nick,
            initials,
            hue,
            photo,
        } = chat.face(&person("ada"))
        else {
            panic!("a person's face");
        };
        assert_eq!(nick, "ada");
        assert_eq!(initials, "AL");
        assert_eq!(hue, nick_degrees("ada"));
        // No photo until the apps say where pictures load from.
        assert_eq!(photo, None);
        let Face::Person { initials, .. } = chat.face(&person("carol-ann")) else {
            panic!("a person's face");
        };
        assert_eq!(initials, "CA");
    }

    #[test]
    fn photos_load_from_the_web_apps_member_page() {
        let mut chat = chat();
        chat.set_picture_site(Some("https://zirc.dev/".into()), None);
        assert_eq!(chat.photo("ada"), None, "no organisation, no photo");
        chat.set_picture_site(Some("https://zirc.dev/".into()), Some("zirc".into()));
        assert_eq!(
            chat.photo("ada").as_deref(),
            Some("https://zirc.dev/m/zirc/@ada?v=7")
        );
        assert_eq!(chat.picture_web(), Some("https://zirc.dev/"));
        // Someone without a picture shows their initials.
        assert_eq!(chat.photo("bob"), None);
        assert_eq!(chat.photo("nobody"), None);
        let Face::Person { photo, .. } = chat.face(&person("ada")) else {
            panic!("a person's face");
        };
        assert_eq!(photo.as_deref(), Some("https://zirc.dev/m/zirc/@ada?v=7"));
    }

    #[test]
    fn an_agents_face_carries_its_owners() {
        let mut chat = chat();
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("zirc".into()));
        let face = chat.face(&Avatar::Agent {
            tool: Some(zirc_proto::CLAUDE_CODE.into()),
            hue: 42,
            owner: "ada".into(),
            session: Some("cc-598aa1".into()),
            picture: Some("reviewer".into()),
        });
        assert_eq!(
            face,
            Face::Agent {
                tool: Some(zirc_proto::CLAUDE_CODE.into()),
                hue: 42,
                owner: "ada".into(),
                owner_initials: "AL".into(),
                owner_hue: nick_degrees("ada"),
                owner_photo: Some("https://zirc.dev/m/zirc/@ada?v=7".into()),
                session: Some("cc-598aa1".into()),
                picture: Some("reviewer".into()),
                ground: Some(42),
            }
        );
        assert_eq!(
            chat.face(&Chat::agents_avatar()),
            Face::App {
                glyph: AGENTS_GLYPH.into()
            }
        );
    }

    #[test]
    fn faces_go_out_as_json_tagged_by_kind() {
        let face = chat().face(&person("bob"));
        let json = serde_json::to_value(&face).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "person",
                "nick": "bob",
                "initials": "B",
                "hue": nick_degrees("bob"),
                "photo": null,
            })
        );
        let agent = Face::Agent {
            tool: None,
            hue: 1,
            owner: "bob".into(),
            owner_initials: "B".into(),
            owner_hue: 2,
            owner_photo: None,
            session: None,
            picture: None,
            ground: None,
        };
        let json = serde_json::to_value(&agent).unwrap();
        assert_eq!(json["kind"], "agent");
        assert_eq!(json["ownerInitials"], "B");
        assert_eq!(json["ownerHue"], 2);
    }
}
