//! What a person does at zirc.dev rather than over IRC, for every app: learning their role
//! in each organisation from `/api/me`, for [`crate::chat::Chat::set_role`]; pressing a
//! card's button; reading how people show
//! ([`zirc_proto::profile`]); and deleting the account, from the phones. The account is
//! otherwise edited on zirc.dev's own page (20 Nov 2026). All as the app's own account, which zirc.dev takes by HTTP Basic.

use reqwest::StatusCode;
use serde::Deserialize;
use zirc_proto::card;
use zirc_proto::login::Account;
use zirc_proto::profile;

use crate::login::{Client, Error};

/// Where the signed-in person's profile is, with their organisations.
pub const ME_PATH: &str = "/api/me";

/// The browser's session at zirc.dev, which a person's app ends with `DELETE`.
pub const SESSION_PATH: &str = "/api/session";

/// The signed-in person's account, which `DELETE` deletes: the phones' Delete account (the
/// user's choice, 20 Nov 2026, as the stores ask of an app that makes accounts).
pub const ACCOUNT_PATH: &str = "/api/account";

/// Someone's role in one of their organisations, as `/api/me` says it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrgRole {
    /// The organisation's short name.
    pub slug: String,
    /// What [`crate::chat::Chat::set_role`] takes. A member's when the web app doesn't say.
    #[serde(default, with = "zirc_proto::role::lenient")]
    pub role: zirc_proto::role::Role,
}

/// The part of `/api/me` this reads.
#[derive(Deserialize)]
struct Me {
    orgs: Vec<OrgRole>,
}

impl Client {
    /// `account`'s owner's role in each of their organisations, or `None` when the web app
    /// doesn't take the account.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn roles(&self, account: &Account) -> Result<Option<Vec<OrgRole>>, Error> {
        let response = self
            .http
            .get(format!("{}{ME_PATH}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        let me: Me = self.checked(response).await?.json().await?;
        Ok(Some(me.orgs))
    }

    /// `account`'s owner's own profile, with their organisations and what they've set for
    /// each alone, or `None` when the web app doesn't take the account.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn me(&self, account: &Account) -> Result<Option<profile::Me>, Error> {
        let response = self
            .http
            .get(format!("{}{ME_PATH}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// How each of `nicks` shows in `account`'s organisation, a person's or a machine's,
    /// for [`crate::chat::Chat::take_profiles`]: those who aren't anyone there are left out.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn profiles(
        &self,
        account: &Account,
        nicks: &[String],
    ) -> Result<profile::Profiles, Error> {
        let url = reqwest::Url::parse_with_params(
            &format!("{}{}", self.web, profile::PROFILES_PATH),
            nicks.iter().map(|nick| ("nick", nick)),
        )
        .map_err(|_| Error::Unavailable {
            web: self.web.clone(),
        })?;
        let response = self
            .http
            .get(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        Ok(self.checked(response).await?.json().await?)
    }

    /// Deletes `account`'s owner's account at zirc.dev, `confirm` being their email address
    /// typed out to say they mean it: they leave each of their organisations, and the account
    /// and every app's sign-in go. Only the person's own account may, never a machine's.
    ///
    /// # Errors
    ///
    /// When the address isn't theirs, they're an organisation's only owner, or zirc.dev
    /// can't be reached or refuses, saying why.
    pub async fn delete_account(&self, account: &Account, confirm: &str) -> Result<(), Error> {
        let response = self
            .http
            .delete(format!("{}{ACCOUNT_PATH}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .json(&serde_json::json!({ "confirm": confirm }))
            .send()
            .await?;
        self.checked(response).await?;
        Ok(())
    }

    /// Ends the zirc.dev browser session that let `account`'s app in, by the hash its login
    /// gave ([`zirc_proto::login::Credentials::web_session`]): signing out of the app signs out
    /// of zirc.dev on the device too. Only the person's own account may, never a machine's.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses, saying why.
    pub async fn end_web_session(&self, account: &Account, session: &str) -> Result<(), Error> {
        let response = self
            .http
            .delete(format!("{}{SESSION_PATH}", self.web))
            .basic_auth(&account.name, Some(&account.password))
            .json(&zirc_proto::login::WebSessionEnd {
                session: session.to_owned(),
            })
            .send()
            .await?;
        self.checked(response).await?;
        Ok(())
    }

    /// Presses a card's button as `account`, in the organisation `org`
    /// ([`zirc_proto::card::press_path`]): zirc.dev relays it to the integration that
    /// posted the card. What the integration said, if anything.
    ///
    /// # Errors
    ///
    /// When zirc.dev or the integration refuses, or can't be reached, saying why: the
    /// error's text is for the app to show on the card.
    pub async fn press(
        &self,
        account: &Account,
        org: &str,
        press: &card::Press,
    ) -> Result<Option<String>, Error> {
        let response = self
            .http
            .post(format!("{}{}", self.web, card::press_path(org)))
            .basic_auth(&account.name, Some(&account.password))
            .json(press)
            .send()
            .await?;
        let response = self.checked(response).await?;
        let pressed: card::Pressed = response.json().await.unwrap_or_default();
        Ok(pressed.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account {
            name: "mattias-mbp".into(),
            password: "secret".into(),
            device_key: false,
        }
    }

    #[tokio::test]
    async fn reads_each_organisations_role() {
        let me = r#"{"email":"a@b.c","name":null,"avatar":null,"orgs":[
            {"slug":"zirc","name":"Zirc","handle":"mattias","role":"owner"},
            {"slug":"other","name":"Other","handle":"mattias"}]}"#;
        let (web, served) = zirc_test_ergo::web::serve(vec![(200, me.into())]);
        let roles = Client::new(&web)
            .unwrap()
            .roles(&account())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            roles,
            [
                OrgRole {
                    slug: "zirc".into(),
                    role: zirc_proto::role::Role::Owner
                },
                OrgRole {
                    slug: "other".into(),
                    role: zirc_proto::role::Role::Member
                },
            ]
        );
        let asked = served.join().unwrap();
        assert!(asked[0].starts_with("GET /api/me "), "{}", asked[0]);
        assert!(
            asked[0]
                .to_ascii_lowercase()
                .contains("authorization: basic "),
            "{}",
            asked[0]
        );
    }

    #[tokio::test]
    async fn presses_a_cards_button_and_says_what_came_of_it() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (200, r#"{"message":"Retrying the failed step"}"#.into()),
            (403, r#"{"error":"only bob may retry it"}"#.into()),
        ]);
        let client = Client::new(&web).unwrap();
        let press = card::Press {
            channel: "#monorepo".into(),
            sender: "zirc-alerts".into(),
            card: "monorepo/889".into(),
            action: "retry".into(),
        };
        let said = client.press(&account(), "zirc", &press).await.unwrap();
        assert_eq!(said.as_deref(), Some("Retrying the failed step"));
        let refused = client.press(&account(), "zirc", &press).await.unwrap_err();
        assert!(
            refused.to_string().contains("only bob may retry it"),
            "{refused}"
        );
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("POST /api/organisations/zirc/cards/actions "),
            "{}",
            asked[0]
        );
        assert!(
            asked[0].ends_with(
                r##"{"channel":"#monorepo","sender":"zirc-alerts","card":"monorepo/889","action":"retry"}"##
            ),
            "{}",
            asked[0]
        );
    }

    #[tokio::test]
    async fn asks_how_nicks_show() {
        let page = r#"{"ada":{"name":"Ada Lovelace","avatar":"3"}}"#;
        let (web, served) = zirc_test_ergo::web::serve(vec![(200, page.into())]);
        let profiles = Client::new(&web)
            .unwrap()
            .profiles(&account(), &["ada".into(), "bob-mbp".into()])
            .await
            .unwrap();
        assert_eq!(profiles["ada"].name, "Ada Lovelace");
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("GET /api/profiles?nick=ada&nick=bob-mbp "),
            "{}",
            asked[0]
        );
    }

    #[tokio::test]
    async fn an_account_the_web_app_doesnt_take_has_no_roles() {
        let (web, _served) = zirc_test_ergo::web::serve(vec![(401, "{}".into())]);
        let roles = Client::new(&web).unwrap().roles(&account()).await.unwrap();
        assert_eq!(roles, None);
    }
}
