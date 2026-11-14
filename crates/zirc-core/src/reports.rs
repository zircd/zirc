//! Reporting and blocking at zirc.dev, for every app ([`zirc_proto::report`]): filing a
//! report, and reading and changing the account's blocklist. All as the app's own account,
//! which zirc.dev takes by HTTP Basic.

use zirc_proto::login::Account;
use zirc_proto::report::{self, BlockList, Filed, Report};

use crate::login::{Client, Error};

impl Client {
    /// Files `report` in the organisation `org` as `account`. The report's id.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses, saying why: the text is for the person.
    pub async fn report(
        &self,
        account: &Account,
        org: &str,
        report: &Report,
    ) -> Result<String, Error> {
        let response = self
            .http
            .post(format!("{}{}", self.web, report::reports_path(org)))
            .basic_auth(&account.name, Some(&account.password))
            .json(report)
            .send()
            .await?;
        let filed: Filed = self.checked(response).await?.json().await?;
        Ok(filed.id)
    }

    /// `account`'s blocklist in the organisation `org`.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses.
    pub async fn blocks(&self, account: &Account, org: &str) -> Result<BlockList, Error> {
        let response = self
            .http
            .get(format!("{}{}", self.web, report::blocks_path(org)))
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        Ok(self.checked(response).await?.json().await?)
    }

    /// Blocks `who`, or with `blocked` false unblocks them, for `account` in `org`.
    ///
    /// # Errors
    ///
    /// When zirc.dev can't be reached or refuses, as for someone who isn't in the
    /// organisation.
    pub async fn set_blocked(
        &self,
        account: &Account,
        org: &str,
        who: &str,
        blocked: bool,
    ) -> Result<(), Error> {
        let url = format!("{}{}", self.web, report::block_path(org, who));
        let request = if blocked {
            self.http.put(url)
        } else {
            self.http.delete(url)
        };
        let response = request
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        self.checked(response).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::report::{Kind, Reason};

    use super::*;

    fn account() -> Account {
        Account {
            name: "mattias-mbp".into(),
            password: "secret".into(),
            device_key: false,
        }
    }

    #[tokio::test]
    async fn files_a_report_and_says_its_id_or_why_not() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (201, r#"{"id":"r1"}"#.into()),
            (429, r#"{"error":"Too many reports"}"#.into()),
        ]);
        let client = Client::new(&web).unwrap();
        let report = Report {
            kind: Kind::Message,
            reason: Reason::Harassment,
            account: "mallory".into(),
            channel: Some("#web".into()),
            message: Some("m1".into()),
            note: None,
        };
        assert_eq!(
            client.report(&account(), "acme", &report).await.unwrap(),
            "r1"
        );
        let refused = client
            .report(&account(), "acme", &report)
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("Too many reports"),
            "{refused}"
        );
        let asked = served.join().unwrap();
        assert!(
            asked[0].starts_with("POST /api/organisations/acme/reports "),
            "{}",
            asked[0]
        );
        assert!(asked[0].ends_with(
            r##"{"kind":"message","reason":"harassment","account":"mallory","channel":"#web","message":"m1"}"##
        ));
    }

    #[tokio::test]
    async fn reads_the_blocklist_and_blocks_and_unblocks() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (
                200,
                r#"{"blocked":[{"account":"mallory","name":"Mal"}]}"#.into(),
            ),
            (200, "{}".into()),
            (200, "{}".into()),
        ]);
        let client = Client::new(&web).unwrap();
        let list = client.blocks(&account(), "acme").await.unwrap();
        assert_eq!(list.blocked[0].account, "mallory");
        assert_eq!(list.blocked[0].name.as_deref(), Some("Mal"));
        client
            .set_blocked(&account(), "acme", "eve", true)
            .await
            .unwrap();
        client
            .set_blocked(&account(), "acme", "eve", false)
            .await
            .unwrap();
        let asked = served.join().unwrap();
        assert!(asked[0].starts_with("GET /api/organisations/acme/blocks "));
        assert!(asked[1].starts_with("PUT /api/organisations/acme/blocks/eve "));
        assert!(asked[2].starts_with("DELETE /api/organisations/acme/blocks/eve "));
    }
}
