//! The one-time notes a person has seen, at zirc.dev, for every app
//! ([`zirc_proto::onboarding::Note`]): which they've seen in any of their apps, and marking
//! one seen, so a note put away in one app is gone in the others. As the app's own account,
//! which zirc.dev takes by HTTP Basic. What shows, and when, is zirc-chat's
//! (`Chat::set_notes_seen`, `Chat::see_note`).

use zirc_proto::login::Account;
use zirc_proto::onboarding::{NOTES_PATH, Note, NoteSeen, NotesSeen};

use crate::login::{Client, Error};

impl Client {
    /// The one-time notes `account`'s person has seen.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses.
    pub async fn notes_seen(&self, account: &Account) -> Result<Vec<Note>, Error> {
        let response = self
            .http
            .get(format!("{}{NOTES_PATH}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        let seen: NotesSeen = self.checked(response).await?.json().await?;
        Ok(seen.seen)
    }

    /// Marks `note` seen by `account`'s person, in every app of theirs.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses.
    pub async fn see_note(&self, account: &Account, note: Note) -> Result<(), Error> {
        let response = self
            .http
            .post(format!("{}{NOTES_PATH}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .json(&NoteSeen { note })
            .send()
            .await?;
        self.checked(response).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_what_was_seen_and_marks_one() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (200, r#"{"seen":["first_wake"]}"#.into()),
            (204, String::new()),
        ]);
        let client = Client::new(&web).unwrap();
        let account = Account {
            name: "mattias".into(),
            password: "secret".into(),
            device_key: false,
        };
        assert_eq!(
            client.notes_seen(&account).await.unwrap(),
            [Note::FirstWake]
        );
        client.see_note(&account, Note::FirstOverlap).await.unwrap();
        let asked = served.join().unwrap();
        assert!(asked[0].starts_with("GET /api/me/notes "), "{}", asked[0]);
        assert!(asked[1].starts_with("POST /api/me/notes "), "{}", asked[1]);
        assert!(
            asked[1].ends_with(r#"{"note":"first_overlap"}"#),
            "{}",
            asked[1]
        );
    }
}
