//! Makes zirc's `PostHog` dashboards in a project, replacing the ones it made before: see
//! [`zirc_core::telemetry::dashboards`]. Needs `POSTHOG_PROJECT_ID` and a personal API key
//! with write access to dashboards and insights in `POSTHOG_PERSONAL_API_KEY`;
//! `POSTHOG_API_HOST` is `https://eu.posthog.com` unless set.

use std::process::ExitCode;

use zirc_core::telemetry::dashboards::{Api, dashboards};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let (Some(project), Some(key)) = (var("POSTHOG_PROJECT_ID"), var("POSTHOG_PERSONAL_API_KEY"))
    else {
        eprintln!("posthog_dashboards: set POSTHOG_PROJECT_ID and POSTHOG_PERSONAL_API_KEY");
        return ExitCode::FAILURE;
    };
    let Some(http) = zirc_core::telemetry::http() else {
        eprintln!("posthog_dashboards: couldn't set up HTTP");
        return ExitCode::FAILURE;
    };
    let api = Api {
        http,
        host: var("POSTHOG_API_HOST").unwrap_or_else(|| "https://eu.posthog.com".to_owned()),
        project,
        key,
    };
    match api.apply(&dashboards()).await {
        Ok(made) => {
            for url in made {
                println!("{url}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("posthog_dashboards: {err}");
            ExitCode::FAILURE
        }
    }
}
