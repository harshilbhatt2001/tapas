//! Files in the hidden Google Drive appDataFolder: bytes and metadata only, no `Store`.

use std::collections::HashMap;
use std::io::Cursor;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use google_drive3::api::{File, Revision};
use google_drive3::{DriveHub, Error as ApiError, common, hyper_util, yup_oauth2};

use super::auth::{self, Auth, DRIVE_SCOPE};

/// The appDataFolder space and parent alias.
const APP_DATA: &str = "appDataFolder";
const SCHEMA_KEY: &str = "tapasSchema";
const DEVICE_KEY: &str = "tapasDevice";
/// Drive v3 returns only id, name and mimeType unless asked for more.
const FILE_FIELDS: &str = "id,headRevisionId,createdTime,appProperties";

/// Remote file metadata; `head_revision_id` names the current content revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMeta {
    pub file_id: String,
    pub head_revision_id: String,
    pub created: DateTime<Utc>,
    /// `tapasSchema` app property: the store `version` that wrote the file.
    pub schema: Option<u32>,
    /// `tapasDevice` app property: the device that wrote the file.
    pub device: Option<String>,
}

impl TryFrom<File> for RemoteMeta {
    type Error = anyhow::Error;

    fn try_from(file: File) -> Result<Self> {
        let props = file.app_properties.unwrap_or_default();
        let schema = props
            .get(SCHEMA_KEY)
            .map(|s| s.parse().with_context(|| format!("bad {SCHEMA_KEY} {s:?}")))
            .transpose()?;
        Ok(Self {
            file_id: file.id.context("Drive file has no id")?,
            head_revision_id: file
                .head_revision_id
                .context("Drive file has no head revision")?,
            created: file.created_time.context("Drive file has no createdTime")?,
            schema,
            device: props.get(DEVICE_KEY).cloned(),
        })
    }
}

/// One content revision of a file, oldest first in [`Drive::revisions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRevision {
    pub id: String,
    pub modified: Option<DateTime<Utc>>,
}

impl TryFrom<Revision> for RemoteRevision {
    type Error = anyhow::Error;

    fn try_from(rev: Revision) -> Result<Self> {
        Ok(Self {
            id: rev.id.context("Drive revision has no id")?,
            modified: rev.modified_time,
        })
    }
}

fn app_properties(schema: u32, device: &str) -> HashMap<String, String> {
    HashMap::from([
        (SCHEMA_KEY.to_owned(), schema.to_string()),
        (DEVICE_KEY.to_owned(), device.to_owned()),
    ])
}

/// Drive `q` matching live files called `name`.
fn name_query(name: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
    format!("name = '{escaped}' and trashed = false")
}

/// Context for a failed call; a refused login becomes just the [`auth::login_hint`].
fn api_err(err: ApiError, what: &str) -> anyhow::Error {
    if let ApiError::MissingToken(e) = &err
        && let Some(yup_oauth2::Error::UserError(hint)) = e.downcast_ref()
    {
        return auth::NeedsLogin(hint.clone()).into();
    }
    anyhow::Error::new(err).context(what.to_owned())
}

/// A Drive client limited to `drive.appdata`: every call adds [`DRIVE_SCOPE`], because the
/// generated defaults ask for full Drive (or unrelated) scopes.
pub struct Drive {
    hub: DriveHub<auth::Connector>,
}

impl Drive {
    /// Pass an [`auth::background_authenticator`] for `Api::Calendar` so a token without the
    /// Drive scope fails with the login hint instead of opening a browser.
    pub fn new(auth: Auth) -> Result<Self> {
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(auth::connector()?);
        Ok(Self {
            hub: DriveHub::new(client, auth),
        })
    }

    /// Every appDataFolder file called `name`, oldest first.
    pub async fn find(&self, name: &str) -> Result<Vec<RemoteMeta>> {
        let q = name_query(name);
        let fields = format!("nextPageToken,files({FILE_FIELDS})");
        let mut found = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut call = self
                .hub
                .files()
                .list()
                .spaces(APP_DATA)
                .q(&q)
                .order_by("createdTime")
                .param("fields", &fields)
                .add_scope(DRIVE_SCOPE);
            if let Some(p) = &page {
                call = call.page_token(p);
            }
            let (_, list) = call
                .doit()
                .await
                .map_err(|e| api_err(e, "listing Drive files"))?;
            for file in list.files.into_iter().flatten() {
                found.push(RemoteMeta::try_from(file)?);
            }
            page = list.next_page_token;
            if page.is_none() {
                return Ok(found);
            }
        }
    }

    /// Content of exactly `revision_id`, so the bytes match the head a listing reported.
    pub async fn download(&self, file_id: &str, revision_id: &str) -> Result<Vec<u8>> {
        let (res, _) = self
            .hub
            .revisions()
            .get(file_id, revision_id)
            .param("alt", "media")
            .add_scope(DRIVE_SCOPE)
            .doit()
            .await
            .map_err(|e| api_err(e, "downloading Drive revision"))?;
        let bytes = common::to_bytes(res.into_body())
            .await
            .context("reading Drive download body")?;
        Ok(bytes.to_vec())
    }

    /// Upload a new JSON file called `name` into appDataFolder.
    pub async fn create(
        &self,
        name: &str,
        bytes: Vec<u8>,
        schema: u32,
        device: &str,
    ) -> Result<RemoteMeta> {
        let file = File {
            name: Some(name.to_owned()),
            parents: Some(vec![APP_DATA.to_owned()]),
            app_properties: Some(app_properties(schema, device)),
            ..Default::default()
        };
        let (_, file) = self
            .hub
            .files()
            .create(file)
            .param("fields", FILE_FIELDS)
            .add_scope(DRIVE_SCOPE)
            .upload(Cursor::new(bytes), mime::APPLICATION_JSON)
            .await
            .map_err(|e| api_err(e, "creating Drive file"))?;
        RemoteMeta::try_from(file)
    }

    /// Replace the content of `file_id`, which adds a new head revision.
    pub async fn update(
        &self,
        file_id: &str,
        bytes: Vec<u8>,
        schema: u32,
        device: &str,
    ) -> Result<RemoteMeta> {
        let file = File {
            app_properties: Some(app_properties(schema, device)),
            ..Default::default()
        };
        let (_, file) = self
            .hub
            .files()
            .update(file, file_id)
            .param("fields", FILE_FIELDS)
            .add_scope(DRIVE_SCOPE)
            .upload(Cursor::new(bytes), mime::APPLICATION_JSON)
            .await
            .map_err(|e| api_err(e, "updating Drive file"))?;
        RemoteMeta::try_from(file)
    }

    /// Delete `file_id` for good (appDataFolder files cannot be trashed).
    pub async fn delete(&self, file_id: &str) -> Result<()> {
        self.hub
            .files()
            .delete(file_id)
            .add_scope(DRIVE_SCOPE)
            .doit()
            .await
            .map_err(|e| api_err(e, "deleting Drive file"))?;
        Ok(())
    }

    /// Content revisions of `file_id` in the order Drive lists them, oldest first in practice
    /// (the API does not promise an order; `sync` sorts by `modified`).
    pub async fn revisions(&self, file_id: &str) -> Result<Vec<RemoteRevision>> {
        let mut revs = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut call = self
                .hub
                .revisions()
                .list(file_id)
                .page_size(1000)
                .param("fields", "nextPageToken,revisions(id,modifiedTime)")
                .add_scope(DRIVE_SCOPE);
            if let Some(p) = &page {
                call = call.page_token(p);
            }
            let (_, list) = call
                .doit()
                .await
                .map_err(|e| api_err(e, "listing Drive revisions"))?;
            for rev in list.revisions.into_iter().flatten() {
                revs.push(RemoteRevision::try_from(rev)?);
            }
            page = list.next_page_token;
            if page.is_none() {
                return Ok(revs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `services::classify` tells a needed login apart by this downcast.
    #[test]
    fn refused_login_becomes_needs_login() {
        let hint = auth::login_hint(auth::Api::Calendar);
        let err = ApiError::MissingToken(Box::new(yup_oauth2::Error::UserError(hint)));
        let err = api_err(err, "listing");
        assert!(err.downcast_ref::<auth::NeedsLogin>().is_some());
    }
}
