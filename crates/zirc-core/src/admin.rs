//! What a person does at zirc.dev rather than over IRC, for every app: learning their role
//! in each organisation from `/api/me`, for [`crate::chat::Chat::set_role`]; making the
//! Guests API's requests that [`crate::chat::Chat`] gives an organisation's owners and
//! admins ([`GuestRequest`]), such as banning a guest; listing the guests it keeps, whose rooms
//! the channels list for them ([`Client::kept_guests`]); and reading how people show
//! ([`zirc_proto::profile`]). The account itself is edited on zirc.dev's own page (20 Nov
//! 2026). All as the app's own account, which zirc.dev takes by HTTP Basic.

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use zirc_proto::card;
use zirc_proto::login::Account;
use zirc_proto::profile;

use crate::chat::{GuestRequest, KeptGuest};
use crate::login::{Client, Error};

/// Where the signed-in person's profile is, with their organisations.
pub const ME_PATH: &str = "/api/me";

/// The browser's session at zirc.dev, which a person's app ends with `DELETE`.
pub const SESSION_PATH: &str = "/api/session";

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

    /// Makes `request` of the Guests API as `account`.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or refuses, saying why, as for someone who isn't
    /// an admin: the error's text is for the app to show.
    pub async fn send(&self, account: &Account, request: &GuestRequest) -> Result<(), Error> {
        let method =
            Method::from_bytes(request.method.as_bytes()).map_err(|_| Error::Unavailable {
                web: self.web.clone(),
            })?;
        let response = self
            .http
            .request(method, format!("{}{}", self.web, request.path))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        self.checked(response).await?;
        Ok(())
    }

    /// The guests the organisation `org` (its slug) keeps, as zirc.dev lists them to its
    /// owners and admins ([`crate::chat::KEPT_GUESTS_PATH`]): what lists their rooms
    /// ([`crate::chat::Chat::set_kept_guests`]). None for anyone else, whom zirc.dev
    /// refuses.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn kept_guests(&self, account: &Account, org: &str) -> Result<Vec<KeptGuest>, Error> {
        let response = self
            .http
            .get(format!(
                "{}{}",
                self.web,
                crate::chat::kept_guests_path(org)
            ))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ) {
            return Ok(Vec::new());
        }
        let text = self.checked(response).await?.text().await?;
        Ok(crate::chat::parse_kept_guests(&text).unwrap_or_default())
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
            {"slug":"acme","name":"Acme","handle":"mattias","role":"owner"},
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
                    slug: "acme".into(),
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
        let said = client.press(&account(), "acme", &press).await.unwrap();
        assert_eq!(said.as_deref(), Some("Retrying the failed step"));
        let refused = client.press(&account(), "acme", &press).await.unwrap_err();
        assert!(
            refused.to_string().contains("only bob may retry it"),
            "{refused}"
        );
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("POST /api/organisations/acme/cards/actions "),
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

    #[tokio::test]
    async fn lists_the_kept_guests_for_owners_and_admins_alone() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (
                200,
                r##"{"guests":[{"nick":"guest-21042","name":"Sam","channels":["#web"],"connected":true}],"settings":{}}"##.into(),
            ),
            (403, r#"{"error":"Your role can't do that here."}"#.into()),
        ]);
        let client = Client::new(&web).unwrap();
        let guests = client.kept_guests(&account(), "acme").await.unwrap();
        assert_eq!(guests.len(), 1);
        assert_eq!(guests[0].rooms(), ["#web/guest-21042"]);
        assert!(
            client
                .kept_guests(&account(), "acme")
                .await
                .unwrap()
                .is_empty()
        );
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("GET /api/organisations/acme/guests "),
            "{}",
            asked[0]
        );
    }

    #[tokio::test]
    async fn sends_a_guest_request_and_says_why_it_was_refused() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (200, "{}".into()),
            (
                403,
                r#"{"error":"Only owners and admins can ban guests."}"#.into(),
            ),
        ]);
        let client = Client::new(&web).unwrap();
        let ban = GuestRequest {
            method: "POST",
            path: "/api/organisations/acme/guests/guest-7k2q/ban".into(),
        };
        client.send(&account(), &ban).await.unwrap();
        let err = client.send(&account(), &ban).await.unwrap_err();
        assert!(
            err.to_string()
                .ends_with("Only owners and admins can ban guests."),
            "{err}"
        );
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("POST /api/organisations/acme/guests/guest-7k2q/ban "),
            "{}",
            asked[0]
        );
    }
}
