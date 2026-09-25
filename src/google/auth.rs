//! OAuth installed flow (browser redirect) with tokens cached on disk.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::{Context, Result};
use google_calendar3::yup_oauth2::authenticator::Authenticator;
use google_calendar3::yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use google_calendar3::yup_oauth2::client::CustomHyperClientBuilder;
use google_calendar3::yup_oauth2::{
    InstalledFlowAuthenticator, InstalledFlowReturnMethod, read_application_secret,
};
use google_calendar3::{hyper_rustls, hyper_util};

pub const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.app.created";
/// Files tapas creates in the hidden Drive appDataFolder, used for store sync.
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
pub const ACTIVITY_SCOPE: &str =
    "https://www.googleapis.com/auth/googlehealth.activity_and_fitness.readonly";
pub const METRICS_SCOPE: &str =
    "https://www.googleapis.com/auth/googlehealth.health_metrics_and_measurements.readonly";
/// A Google API with its own consent and token cache: the Health API rejects tokens that
/// also carry another API's scopes (`DISALLOWED_OAUTH_SCOPES`). Drive rides on the Calendar
/// consent and token cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Calendar,
    Health,
}

impl Api {
    pub const ALL: [Api; 2] = [Api::Calendar, Api::Health];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Api::Calendar => "calendar",
            Api::Health => "health",
        }
    }

    #[must_use]
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Api::Calendar => &[CALENDAR_SCOPE, DRIVE_SCOPE],
            Api::Health => &[ACTIVITY_SCOPE, METRICS_SCOPE],
        }
    }
}

/// Downloaded client JSONs still point at the legacy `/o/oauth2/auth` endpoint, which rejects
/// newer scopes such as `googlehealth.*`; the v2 endpoint accepts every catalogued scope.
const AUTH_URI: &str = "https://accounts.google.com/o/oauth2/v2/auth";

pub type Connector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;
pub type Auth = Authenticator<Connector>;

/// HTTPS connector shared by the authenticator and the Calendar hub.
pub(crate) fn connector() -> Result<Connector> {
    Ok(hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .context("loading native TLS roots")?
        .https_only()
        .enable_http2()
        .build())
}

/// Prints the consent URL and opens it in the default browser.
struct BrowserDelegate;

impl InstalledFlowDelegate for BrowserDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            println!("Opening Google consent page in your browser:\n{url}");
            if let Err(e) = open::that(url) {
                println!("Could not open a browser ({e}); open the URL above manually.");
            }
            Ok(String::new())
        })
    }
}

/// Refuses the consent flow so background calls never prompt. In `Interactive` mode yup-oauth2
/// aborts on this `Err`; `HTTPRedirect` would ignore it and wait for a redirect.
struct NoPromptDelegate(Api);

impl InstalledFlowDelegate for NoPromptDelegate {
    fn present_user_url<'a>(
        &'a self,
        _url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move { Err(login_hint(self.0)) })
    }
}

/// Why a non-interactive token request failed and how to fix it.
#[must_use]
pub fn login_hint(api: Api) -> String {
    let name = api.name();
    format!(
        "Google {name} needs a new login (no saved token, a missing scope, or the refresh \
         failed): run `tapas google login --only {name}`"
    )
}

/// Build an authenticator from a Google "Desktop app" client JSON, caching tokens at `tokens`.
/// A missing or insufficient token opens the browser consent flow.
pub async fn authenticator(secret: &Path, tokens: &Path) -> Result<Auth> {
    build(
        secret,
        tokens,
        InstalledFlowReturnMethod::HTTPRedirect,
        Box::new(BrowserDelegate),
    )
    .await
}

/// Like [`authenticator`], but never prompts: a saved refresh token refreshes silently, anything
/// else fails with [`login_hint`]. For the TUI and background calls.
pub async fn background_authenticator(secret: &Path, api: Api, tokens: &Path) -> Result<Auth> {
    build(
        secret,
        tokens,
        InstalledFlowReturnMethod::Interactive,
        Box::new(NoPromptDelegate(api)),
    )
    .await
}

async fn build(
    secret: &Path,
    tokens: &Path,
    method: InstalledFlowReturnMethod,
    delegate: Box<dyn InstalledFlowDelegate>,
) -> Result<Auth> {
    let mut app_secret = read_application_secret(secret)
        .await
        .with_context(|| format!("reading OAuth client {}", secret.display()))?;
    AUTH_URI.clone_into(&mut app_secret.auth_uri);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector()?);
    InstalledFlowAuthenticator::with_client(
        app_secret,
        method,
        CustomHyperClientBuilder::from(client),
    )
    .persist_tokens_to_disk(tokens)
    .flow_delegate(delegate)
    .build()
    .await
    .context("building OAuth authenticator")
}

/// Run the consent flow for all of `api`'s scopes so later calls reuse the cached token.
pub async fn login(secret: &Path, api: Api, tokens: &Path) -> Result<Auth> {
    let auth = authenticator(secret, tokens).await?;
    access_token(&auth, api.scopes()).await?;
    Ok(auth)
}

/// Current access token for `scopes`, refreshing or prompting as needed.
pub async fn access_token(auth: &Auth, scopes: &[&str]) -> Result<String> {
    let token = auth.token(scopes).await.context("fetching access token")?;
    token
        .token()
        .map(str::to_owned)
        .context("Google returned no access token")
}

/// True when the token cache holds at least one token.
#[must_use]
pub fn is_logged_in(tokens: &Path) -> bool {
    std::fs::read(tokens)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<serde_json::Value>>(&b).ok())
        .is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_in_needs_non_empty_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        assert!(!is_logged_in(&path));
        std::fs::write(&path, "[]").unwrap();
        assert!(!is_logged_in(&path));
        std::fs::write(&path, r#"[{"scopes":["x"],"token":{}}]"#).unwrap();
        assert!(is_logged_in(&path));
    }
}
