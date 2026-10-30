//! Google OAuth for the server's *Web application* client: exchanging the app's server auth code
//! and refreshing access tokens.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// A Google API with its own grant. The Health API rejects access tokens that also carry other
/// APIs' scopes (`DISALLOWED_OAUTH_SCOPES`), so each API is consented and refreshed apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoogleApi {
    Health,
    Calendar,
}

impl GoogleApi {
    pub const ALL: [Self; 2] = [Self::Health, Self::Calendar];

    #[must_use]
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Health => &[
                "https://www.googleapis.com/auth/googlehealth.activity_and_fitness.readonly",
                "https://www.googleapis.com/auth/googlehealth.health_metrics_and_measurements.readonly",
            ],
            Self::Calendar => &[
                "https://www.googleapis.com/auth/calendar.app.created",
                "https://www.googleapis.com/auth/calendar.freebusy",
            ],
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Calendar => "calendar",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Google OAuth request failed")]
    Http(#[from] reqwest::Error),
    /// The refresh token was revoked or expired: only a new consent in the app fixes it.
    #[error("Google refused the grant; connect the account again in the app")]
    InvalidGrant,
    #[error("Google OAuth returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("Google granted no refresh token; revoke tapas's access and connect again")]
    NoRefreshToken,
}

pub struct WebClient {
    pub id: String,
    pub secret: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

pub struct Grant {
    pub refresh_token: String,
    pub access_token: String,
}

impl WebClient {
    /// Exchanges a server auth code the Android app obtained with this client's id as
    /// `serverClientId`. Such codes carry no redirect URI, so Google expects an empty one.
    pub async fn exchange_code(&self, http: &reqwest::Client, code: &str) -> Result<Grant, Error> {
        let tokens = self
            .token_request(
                http,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("redirect_uri", ""),
                ],
            )
            .await?;
        Ok(Grant {
            refresh_token: tokens.refresh_token.ok_or(Error::NoRefreshToken)?,
            access_token: tokens.access_token,
        })
    }

    pub async fn access_token(
        &self,
        http: &reqwest::Client,
        refresh_token: &str,
    ) -> Result<String, Error> {
        let tokens = self
            .token_request(
                http,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ],
            )
            .await?;
        Ok(tokens.access_token)
    }

    async fn token_request(
        &self,
        http: &reqwest::Client,
        params: &[(&str, &str)],
    ) -> Result<TokenResponse, Error> {
        let client = [
            ("client_id", self.id.as_str()),
            ("client_secret", self.secret.as_str()),
        ];
        let form: Vec<_> = client.iter().chain(params).collect();
        let response = http.post(TOKEN_URL).form(&form).send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.json().await?);
        }
        let body = response.text().await?;
        if body.contains("\"invalid_grant\"") {
            return Err(Error::InvalidGrant);
        }
        Err(Error::Status { status, body })
    }
}
