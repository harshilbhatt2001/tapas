//! Sign-in: the app sends a Google ID token, the server checks it belongs to the one allowed
//! account and answers with an opaque session token for `Authorization: Bearer`.

use anyhow::{Context, Result, bail};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::api::{ApiError, AppState};

const GOOGLE_CERTS: &str = "https://www.googleapis.com/oauth2/v3/certs";
const SESSION_LIFETIME: Duration = Duration::days(30);

pub struct IdTokenVerifier {
    /// The Web client's id: the app requests ID tokens with it as `serverClientId`.
    pub audience: String,
    pub allowed_email: String,
    keys: RwLock<JwkSet>,
}

#[derive(Deserialize)]
struct Claims {
    email: String,
    #[serde(default)]
    email_verified: bool,
}

impl IdTokenVerifier {
    pub fn new(audience: String, allowed_email: String) -> Self {
        Self {
            audience,
            allowed_email,
            keys: RwLock::new(JwkSet { keys: Vec::new() }),
        }
    }

    pub async fn verify(&self, http: &reqwest::Client, id_token: &str) -> Result<()> {
        let kid = decode_header(id_token)?
            .kid
            .context("ID token has no key id")?;
        let key = if let Some(key) = self.key(&kid).await? {
            key
        } else {
            *self.keys.write().await = http.get(GOOGLE_CERTS).send().await?.json().await?;
            self.key(&kid)
                .await?
                .context("ID token signed by an unknown Google key")?
        };
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&["accounts.google.com", "https://accounts.google.com"]);
        let claims = decode::<Claims>(id_token, &key, &validation)?.claims;
        if !claims.email_verified || !claims.email.eq_ignore_ascii_case(&self.allowed_email) {
            bail!("{} may not sign in", claims.email);
        }
        Ok(())
    }

    async fn key(&self, kid: &str) -> Result<Option<DecodingKey>> {
        let keys = self.keys.read().await;
        Ok(keys.find(kid).map(DecodingKey::from_jwk).transpose()?)
    }
}

/// A new random session token, and the hash the store keeps.
pub async fn issue_session(state: &AppState) -> Result<String> {
    let token = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    state
        .store
        .insert_session(&hash(&token), Utc::now() + SESSION_LIFETIME)
        .await?;
    Ok(token)
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Proof that the request carried a valid session token.
pub struct Session {
    pub token_hash: Vec<u8>,
}

impl FromRequestParts<AppState> for Session {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        let token_hash = hash(token);
        if state.store.session_valid(&token_hash, Utc::now()).await? {
            Ok(Session { token_hash })
        } else {
            Err(ApiError::Unauthorized)
        }
    }
}
