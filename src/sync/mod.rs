//! Store sync between machines through one `store.json` in the Drive appDataFolder.
//!
//! This machine keeps, under `<data_dir>/sync/` and never synced: `state.json` (the Drive file
//! and the head revision it last synced) and `base.json` (that revision's document). The store
//! is dirty when it differs from the base. [`sync`] pushes, fast-forwards or three-way merges
//! ([`merge::merge`]) and then checks its upload against Drive's revision list. Drive is reached
//! only through [`Remote`], so the decisions run against an in-memory fake in tests.

pub mod merge;

use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    google::drive::{Drive, RemoteMeta, RemoteRevision},
    model::{DAYS, Device, Library, STORE_VERSION, Store},
    storage::{self, Paths},
};
use merge::{Conflict, Entity, Merged, Side};

/// Name of the synced file in appDataFolder.
pub const FILE_NAME: &str = "store.json";
/// Uploads per sync before giving up on a Drive file that keeps changing under us.
const MAX_UPLOADS: usize = 3;

/// What this machine last synced, in `sync/state.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SyncState {
    pub file_id: String,
    /// The Drive head revision whose document is `sync/base.json`.
    pub base_head: String,
    pub synced_at: DateTime<Utc>,
}

/// The Drive calls sync makes; [`Drive`] in production.
pub trait Remote {
    /// Every appDataFolder file called `name`, oldest first.
    fn find(&self, name: &str) -> impl Future<Output = Result<Vec<RemoteMeta>>> + Send;
    fn download(
        &self,
        file_id: &str,
        revision_id: &str,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn create(
        &self,
        name: &str,
        bytes: Vec<u8>,
        schema: u32,
        device: &str,
    ) -> impl Future<Output = Result<RemoteMeta>> + Send;
    fn update(
        &self,
        file_id: &str,
        bytes: Vec<u8>,
        schema: u32,
        device: &str,
    ) -> impl Future<Output = Result<RemoteMeta>> + Send;
    fn revisions(&self, file_id: &str) -> impl Future<Output = Result<Vec<RemoteRevision>>> + Send;
    fn delete(&self, file_id: &str) -> impl Future<Output = Result<()>> + Send;
}

impl Remote for Drive {
    fn find(&self, name: &str) -> impl Future<Output = Result<Vec<RemoteMeta>>> + Send {
        Drive::find(self, name)
    }
    fn download(
        &self,
        file_id: &str,
        revision_id: &str,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send {
        Drive::download(self, file_id, revision_id)
    }
    fn create(
        &self,
        name: &str,
        bytes: Vec<u8>,
        schema: u32,
        device: &str,
    ) -> impl Future<Output = Result<RemoteMeta>> + Send {
        Drive::create(self, name, bytes, schema, device)
    }
    fn update(
        &self,
        file_id: &str,
        bytes: Vec<u8>,
        schema: u32,
        device: &str,
    ) -> impl Future<Output = Result<RemoteMeta>> + Send {
        Drive::update(self, file_id, bytes, schema, device)
    }
    fn revisions(&self, file_id: &str) -> impl Future<Output = Result<Vec<RemoteRevision>>> + Send {
        Drive::revisions(self, file_id)
    }
    fn delete(&self, file_id: &str) -> impl Future<Output = Result<()>> + Send {
        Drive::delete(self, file_id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Neither side changed.
    UpToDate,
    /// Local changes uploaded.
    Pushed,
    /// Drive's changes taken over a clean local store.
    Pulled,
    /// Nothing on Drive yet: this store was uploaded.
    Created,
    /// Both sides changed: merged, saved and uploaded. `conflicts` are edits that lost.
    Merged { conflicts: Vec<Conflict> },
    /// `--keep`: this side overwrote the other.
    Kept(Side),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub outcome: Outcome,
    /// Copies saved before they were overwritten: the local store when a merge dropped an edit
    /// or on `--keep remote`, Drive's on `--keep local`.
    pub conflict_files: Vec<PathBuf>,
    /// Worth telling the user, but the sync went through.
    pub warnings: Vec<String>,
}

/// How Drive's copy relates to what this machine last synced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drift {
    /// No store on Drive.
    Missing,
    Unchanged,
    /// Another machine uploaded since.
    Changed,
    /// A file this machine never synced (first sync, or it was replaced).
    Untracked,
}

fn state_file(paths: &Paths) -> PathBuf {
    paths.sync_dir().join("state.json")
}

fn base_file(paths: &Paths) -> PathBuf {
    paths.sync_dir().join("base.json")
}

fn read_file<T>(path: &Path, parse: impl FnOnce(&[u8]) -> Result<T>) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => parse(&bytes)
            .map(Some)
            .with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// What this machine last synced; `None` before the first sync.
pub fn read_state(paths: &Paths) -> Result<Option<SyncState>> {
    read_file(&state_file(paths), |b| Ok(serde_json::from_slice(b)?))
}

/// The last synced document.
fn read_base(paths: &Paths) -> Result<Option<Store>> {
    read_file(&base_file(paths), storage::parse)
}

fn encode(store: &Store) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(store)?)
}

/// `meta`'s head now holds `base`: the next sync diffs against it. The base goes first, so a
/// crash in between leaves a state that only costs an extra merge.
fn record(paths: &Paths, meta: &RemoteMeta, base: &Store) -> Result<()> {
    storage::write_atomic(&base_file(paths), &encode(base)?)?;
    let state = SyncState {
        file_id: meta.file_id.clone(),
        base_head: meta.head_revision_id.clone(),
        synced_at: Utc::now(),
    };
    storage::write_atomic(&state_file(paths), &serde_json::to_vec_pretty(&state)?)
}

/// Save `store` to a new `sync/conflict-<utc>.json`.
pub fn save_conflict(paths: &Paths, store: &Store) -> Result<PathBuf> {
    let dir = paths.sync_dir();
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let mut path = dir.join(format!("conflict-{stamp}.json"));
    for n in 2.. {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("conflict-{stamp}-{n}.json"));
    }
    storage::write_atomic(&path, &encode(store)?)?;
    Ok(path)
}

/// Local changes that are not on Drive yet: the store differs from the last synced document.
/// Before the first sync, any saved store counts, so a fresh machine just takes Drive's.
pub fn is_dirty(paths: &Paths, local: &Store) -> Result<bool> {
    Ok(match read_base(paths)? {
        Some(base) => *local != base,
        None => paths.store_file.exists(),
    })
}

/// Checked before anything is downloaded or written, so a file of another version is never
/// overwritten.
fn check_schema(f: &RemoteMeta) -> Result<()> {
    if f.schema != Some(STORE_VERSION) {
        let v = f
            .schema
            .map_or_else(|| "none".to_owned(), |v| v.to_string());
        bail!(
            "the store on Drive has schema {v}, but this tapas reads only version \
             {STORE_VERSION}"
        );
    }
    Ok(())
}

fn remote_device(f: &RemoteMeta) -> Option<Uuid> {
    f.device.as_deref().and_then(|d| Uuid::parse_str(d).ok())
}

/// Merge base without a common ancestor: default settings and nothing else, so every plan,
/// item and type on either side counts as added.
fn empty_base() -> Store {
    Store {
        plans: Vec::new(),
        library: Library { types: Vec::new() },
        ..Store::default()
    }
}

/// Ids of the revisions uploaded after `base` and before `ours`, oldest first; `None` when
/// either is missing from the list, so the upload cannot be checked.
///
/// Drive does not document the order of `revisions.list`; it is oldest first in practice. So
/// sort by `modifiedTime` when every revision has one (stable, keeping list order on ties),
/// else keep the list order. Drive keeps a binary file's revisions for 30 days or 100
/// revisions, and `base` was the head moments ago, so it is still listed unless ordering is
/// off, in which case nothing is merged rather than an older revision.
fn written_between(revs: &[RemoteRevision], base: &str, ours: &str) -> Option<Vec<String>> {
    let mut revs: Vec<&RemoteRevision> = revs.iter().collect();
    if revs.iter().all(|r| r.modified.is_some()) {
        revs.sort_by_key(|r| r.modified);
    }
    let pos = |id: &str| revs.iter().position(|r| r.id == id);
    let (b, o) = (pos(base)?, pos(ours)?);
    (b < o).then(|| revs[b + 1..o].iter().map(|r| r.id.clone()).collect())
}

/// One sync or keep run: the remote, and what to report.
struct Session<'a, R> {
    paths: &'a Paths,
    remote: &'a R,
    device: Uuid,
    merged: bool,
    conflicts: Vec<Conflict>,
    conflict_files: Vec<PathBuf>,
    warnings: Vec<String>,
}

impl<'a, R: Remote> Session<'a, R> {
    fn new(paths: &'a Paths, remote: &'a R, device: &Device) -> Self {
        Session {
            paths,
            remote,
            device: device.device_id,
            merged: false,
            conflicts: Vec::new(),
            conflict_files: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// A clean push or pull that took a merge along the way reports the merge.
    fn finish(self, outcome: Outcome) -> Report {
        let outcome = match outcome {
            Outcome::UpToDate | Outcome::Pushed | Outcome::Pulled if self.merged => {
                Outcome::Merged {
                    conflicts: self.conflicts,
                }
            }
            Outcome::Merged { .. } => Outcome::Merged {
                conflicts: self.conflicts,
            },
            o => o,
        };
        Report {
            outcome,
            conflict_files: self.conflict_files,
            warnings: self.warnings,
        }
    }

    /// Revision `rev` of `file_id`.
    async fn fetch(&self, file_id: &str, rev: &str) -> Result<Store> {
        let bytes = self.remote.download(file_id, rev).await?;
        storage::parse(&bytes).context("reading the store on Drive")
    }

    fn merge(
        &mut self,
        base: &Store,
        local: &Store,
        remote: &Store,
        remote_device: Option<Uuid>,
    ) -> Merged {
        let mut m = merge::merge(base, local, remote, self.device, remote_device);
        self.merged = true;
        self.conflicts.extend(m.conflicts.iter().cloned());
        m.store.normalize();
        m
    }

    fn save_conflict(&mut self, store: &Store) -> Result<()> {
        self.conflict_files.push(save_conflict(self.paths, store)?);
        Ok(())
    }

    /// Replace the local store with `next`, saving the old one first when `save_old`.
    fn replace(&mut self, local: &mut Store, next: Store, save_old: bool) -> Result<()> {
        if *local == next {
            return Ok(());
        }
        if save_old {
            self.save_conflict(local)?;
        }
        storage::write_store(self.paths, &next)?;
        *local = next;
        Ok(())
    }

    async fn create(&mut self, local: &Store) -> Result<()> {
        let device = self.device.to_string();
        let meta = self
            .remote
            .create(FILE_NAME, encode(local)?, STORE_VERSION, &device)
            .await?;
        record(self.paths, &meta, local)
    }

    /// Upload `local` over `file_id`, whose head revision `head` holds `head_doc`, and record
    /// it. Drive has no conditional update, so then check that no other upload landed between
    /// `head` and ours: if one did, ours replaced it, so merge it in and upload again.
    async fn upload(
        &mut self,
        file_id: &str,
        mut head: String,
        mut head_doc: Store,
        local: &mut Store,
    ) -> Result<()> {
        let device = self.device.to_string();
        for _ in 0..MAX_UPLOADS {
            let meta = self
                .remote
                .update(file_id, encode(local)?, STORE_VERSION, &device)
                .await?;
            record(self.paths, &meta, local)?;
            let revs = self.remote.revisions(file_id).await?;
            let Some(between) = written_between(&revs, &head, &meta.head_revision_id) else {
                self.warnings
                    .push("could not check the upload against Drive's revision list".into());
                return Ok(());
            };
            // The latest one: each uploader merges what it replaced, so it holds the rest.
            let Some(lost) = between.last() else {
                return Ok(());
            };
            let theirs = self.fetch(file_id, lost).await?;
            let merged = self.merge(&head_doc, local, &theirs, None).store;
            if merged == *local {
                return Ok(());
            }
            head = meta.head_revision_id;
            head_doc = local.clone();
            self.replace(local, merged, false)?;
        }
        bail!(
            "Drive kept changing during sync; the merged store is saved here and goes up with \
             the next sync"
        )
    }
}

/// Bring the local store and Drive's `store.json` together; `local` is the saved store and is
/// replaced (and saved) when Drive had changes.
///
/// - no file on Drive: upload `local`
/// - Drive unchanged since the last sync: upload `local` if dirty, else nothing
/// - Drive changed, `local` clean: take Drive's
/// - both changed: three-way merge against the base, save, upload
///
/// When a merge dropped an edit, the local store is saved to `sync/conflict-<utc>.json` before
/// it is replaced.
/// Several `store.json` files (two machines' first syncs racing) resolve to the oldest; the
/// others are merged in and deleted. Nothing is written while Drive holds another schema.
pub async fn sync<R: Remote>(
    paths: &Paths,
    remote: &R,
    local: &mut Store,
    device: &Device,
) -> Result<Report> {
    let state = read_state(paths)?;
    let base = read_base(paths)?;
    let files = remote.find(FILE_NAME).await?;
    files.iter().try_for_each(check_schema)?;
    let mut s = Session::new(paths, remote, device);
    let Some((primary, extras)) = files.split_first() else {
        s.create(local).await?;
        return Ok(s.finish(Outcome::Created));
    };

    let base_of = |f: &RemoteMeta| {
        state
            .as_ref()
            .filter(|st| st.file_id == f.file_id)
            .and(base.as_ref())
    };
    let unchanged = |f: &RemoteMeta| {
        base_of(f).is_some()
            && state
                .as_ref()
                .is_some_and(|st| st.base_head == f.head_revision_id)
    };
    let empty = empty_base();
    let dirty = match &base {
        Some(b) => local != b,
        None => paths.store_file.exists(),
    };

    let mut with_extras = local.clone();
    if !extras.is_empty() {
        s.warnings.push(format!(
            "found {} copies of {FILE_NAME} on Drive; merged them into the oldest",
            files.len()
        ));
    }
    let mut dropped_edit = false;
    for f in extras.iter().filter(|f| !unchanged(f)) {
        let doc = s.fetch(&f.file_id, &f.head_revision_id).await?;
        let m = s.merge(
            base_of(f).unwrap_or(&empty),
            &with_extras,
            &doc,
            remote_device(f),
        );
        dropped_edit |= !m.conflicts.is_empty();
        with_extras = m.store;
    }
    // Extra copies are deleted below, so what they held counts as a local change.
    let local_changed = dirty || !extras.is_empty();

    let (file_id, head) = (&primary.file_id, &primary.head_revision_id);
    let outcome = match (unchanged(primary), local_changed, base_of(primary)) {
        (true, false, Some(b)) => {
            record(paths, primary, b)?;
            Outcome::UpToDate
        }
        (true, true, Some(b)) => {
            s.replace(local, with_extras, dropped_edit)?;
            s.upload(file_id, head.clone(), b.clone(), local).await?;
            Outcome::Pushed
        }
        (_, false, _) => {
            let doc = s.fetch(file_id, head).await?;
            let mut next = doc.clone();
            next.normalize();
            s.replace(local, next, false)?;
            record(paths, primary, &doc)?;
            Outcome::Pulled
        }
        (_, true, b) => {
            let doc = s.fetch(file_id, head).await?;
            let m = s.merge(
                b.unwrap_or(&empty),
                &with_extras,
                &doc,
                remote_device(primary),
            );
            dropped_edit |= !m.conflicts.is_empty();
            s.replace(local, m.store, dropped_edit)?;
            if *local == doc {
                record(paths, primary, &doc)?;
            } else {
                s.upload(file_id, head.clone(), doc, local).await?;
            }
            Outcome::Merged {
                conflicts: Vec::new(),
            }
        }
    };
    for f in extras {
        remote.delete(&f.file_id).await?;
    }
    Ok(s.finish(outcome))
}

/// Make both sides `side`'s copy, saving the other side's to `sync/conflict-<utc>.json` first.
/// Extra `store.json` copies on Drive are saved the same way and deleted.
pub async fn keep<R: Remote>(
    paths: &Paths,
    remote: &R,
    local: &mut Store,
    device: &Device,
    side: Side,
) -> Result<Report> {
    let files = remote.find(FILE_NAME).await?;
    files.iter().try_for_each(check_schema)?;
    let mut s = Session::new(paths, remote, device);
    let Some((primary, extras)) = files.split_first() else {
        if side == Side::Remote {
            bail!("there is no store on Drive yet; `tapas sync` uploads this one");
        }
        s.create(local).await?;
        return Ok(s.finish(Outcome::Kept(side)));
    };
    for f in extras {
        let doc = s.fetch(&f.file_id, &f.head_revision_id).await?;
        s.save_conflict(&doc)?;
    }
    let (file_id, head) = (&primary.file_id, &primary.head_revision_id);
    let theirs = s.fetch(file_id, head).await?;
    match side {
        Side::Local => {
            if theirs != *local {
                s.save_conflict(&theirs)?;
            }
            s.upload(file_id, head.clone(), theirs, local).await?;
        }
        Side::Remote => {
            let mut next = theirs.clone();
            next.normalize();
            s.replace(local, next, true)?;
            record(paths, primary, &theirs)?;
        }
    }
    for f in extras {
        remote.delete(&f.file_id).await?;
    }
    Ok(s.finish(Outcome::Kept(side)))
}

/// How Drive's copy relates to the last sync, and every `store.json` on Drive. Writes nothing.
pub async fn remote_status<R: Remote>(
    paths: &Paths,
    remote: &R,
) -> Result<(Drift, Vec<RemoteMeta>)> {
    let state = read_state(paths)?;
    let files = remote.find(FILE_NAME).await?;
    let drift = match (files.first(), &state) {
        (None, _) => Drift::Missing,
        (Some(f), Some(st)) if st.file_id == f.file_id => {
            if st.base_head == f.head_revision_id {
                Drift::Unchanged
            } else {
                Drift::Changed
            }
        }
        (Some(_), _) => Drift::Untracked,
    };
    Ok((drift, files))
}

/// `Tue Run in "Base": kept the other machine's edit`, naming things from `store`.
#[must_use]
pub fn describe(c: &Conflict, store: &Store) -> String {
    let plan = |id: &str| store.plans.iter().find(|p| p.id == id);
    let plan_name =
        |id: &str| plan(id).map_or_else(|| id.to_owned(), |p| format!("\"{}\"", p.name));
    let what = match &c.entity {
        Entity::Plan(id) => format!("plan {}", plan_name(id)),
        Entity::Item { plan: id, item } => match plan(id).and_then(|p| Some((p, p.find(item)?))) {
            Some((p, (d, k))) => {
                let key = &p.days[d][k].type_key;
                let label = store.library.get(key).map_or(key.as_str(), |t| &t.label);
                format!("{} {label} in \"{}\"", DAYS[d], p.name)
            }
            None => format!("a session in plan {}", plan_name(id)),
        },
        Entity::SessionType(key) => format!("session type {key}"),
        Entity::Profile => "profile".into(),
        Entity::Export => "export settings".into(),
        Entity::CalendarId => "Google calendar".into(),
    };
    let kept = match c.kept {
        Side::Local => "this machine's",
        Side::Remote => "the other machine's",
    };
    format!("{what}: kept {kept} edit")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::library::starter_plan;

    /// One Drive file: metadata and every revision, oldest first.
    struct FakeFile {
        meta: RemoteMeta,
        revs: Vec<(RemoteRevision, Vec<u8>)>,
    }

    /// An edit another device makes to the head it sees.
    type Race = Box<dyn FnOnce(&mut Store) + Send>;

    /// appDataFolder in memory. `races` are uploads by another device that land between our
    /// head check and our next update, one per update, in order.
    #[derive(Default)]
    struct Fake {
        files: Mutex<Vec<FakeFile>>,
        races: Mutex<Vec<Race>>,
        next: Mutex<u32>,
    }

    impl Fake {
        fn id(&self, prefix: &str) -> String {
            let mut n = self.next.lock().unwrap();
            *n += 1;
            format!("{prefix}{n}")
        }

        fn at(&self) -> DateTime<Utc> {
            DateTime::from_timestamp(i64::from(*self.next.lock().unwrap()), 0).unwrap()
        }

        fn put(&self, file: &mut FakeFile, bytes: Vec<u8>, schema: u32, device: &str) {
            let rev = RemoteRevision {
                id: self.id("r"),
                modified: Some(self.at()),
            };
            file.meta.head_revision_id.clone_from(&rev.id);
            file.meta.schema = Some(schema);
            file.meta.device = Some(device.to_owned());
            file.revs.push((rev, bytes));
        }

        fn add_file(&self, store: &Store, device: &str) -> RemoteMeta {
            let mut f = FakeFile {
                meta: RemoteMeta {
                    file_id: self.id("f"),
                    head_revision_id: String::new(),
                    created: self.at(),
                    schema: None,
                    device: None,
                },
                revs: Vec::new(),
            };
            self.put(&mut f, encode(store).unwrap(), STORE_VERSION, device);
            let meta = f.meta.clone();
            self.files.lock().unwrap().push(f);
            meta
        }

        /// Another device's upload straight to the head.
        fn upload(&self, i: usize, store: &Store) {
            let mut files = self.files.lock().unwrap();
            let bytes = encode(store).unwrap();
            let f = &mut files[i];
            self.put(f, bytes, STORE_VERSION, &Uuid::from_u128(9).to_string());
        }

        fn head(&self, i: usize) -> Store {
            let files = self.files.lock().unwrap();
            storage::parse(&files[i].revs.last().unwrap().1).unwrap()
        }

        fn revs(&self, i: usize) -> usize {
            self.files.lock().unwrap()[i].revs.len()
        }

        fn count(&self) -> usize {
            self.files.lock().unwrap().len()
        }
    }

    fn ready<T: Send>(t: T) -> impl Future<Output = T> + Send {
        std::future::ready(t)
    }

    impl Remote for Fake {
        fn find(&self, name: &str) -> impl Future<Output = Result<Vec<RemoteMeta>>> + Send {
            assert_eq!(name, FILE_NAME);
            let mut v: Vec<_> = self
                .files
                .lock()
                .unwrap()
                .iter()
                .map(|f| f.meta.clone())
                .collect();
            v.sort_by_key(|m| m.created);
            ready(Ok(v))
        }
        fn download(
            &self,
            file_id: &str,
            revision_id: &str,
        ) -> impl Future<Output = Result<Vec<u8>>> + Send {
            let files = self.files.lock().unwrap();
            let bytes = files
                .iter()
                .find(|f| f.meta.file_id == file_id)
                .and_then(|f| f.revs.iter().find(|(r, _)| r.id == revision_id))
                .map(|(_, b)| b.clone())
                .context("no such revision");
            ready(bytes)
        }
        fn create(
            &self,
            _name: &str,
            bytes: Vec<u8>,
            _schema: u32,
            device: &str,
        ) -> impl Future<Output = Result<RemoteMeta>> + Send {
            let store = storage::parse(&bytes).unwrap();
            ready(Ok(self.add_file(&store, device)))
        }
        fn update(
            &self,
            file_id: &str,
            bytes: Vec<u8>,
            schema: u32,
            device: &str,
        ) -> impl Future<Output = Result<RemoteMeta>> + Send {
            let i = self
                .files
                .lock()
                .unwrap()
                .iter()
                .position(|f| f.meta.file_id == file_id)
                .unwrap();
            let race = {
                let mut races = self.races.lock().unwrap();
                (!races.is_empty()).then(|| races.remove(0))
            };
            if let Some(edit) = race {
                let mut theirs = self.head(i);
                edit(&mut theirs);
                self.upload(i, &theirs);
            }
            let mut files = self.files.lock().unwrap();
            let f = &mut files[i];
            self.put(f, bytes, schema, device);
            ready(Ok(f.meta.clone()))
        }
        fn revisions(
            &self,
            file_id: &str,
        ) -> impl Future<Output = Result<Vec<RemoteRevision>>> + Send {
            let files = self.files.lock().unwrap();
            let f = files.iter().find(|f| f.meta.file_id == file_id).unwrap();
            ready(Ok(f.revs.iter().map(|(r, _)| r.clone()).collect()))
        }
        fn delete(&self, file_id: &str) -> impl Future<Output = Result<()>> + Send {
            self.files
                .lock()
                .unwrap()
                .retain(|f| f.meta.file_id != file_id);
            ready(Ok(()))
        }
    }

    /// One machine: its own data dir, device and loaded store.
    struct Machine {
        _dir: tempfile::TempDir,
        paths: Paths,
        device: Device,
        store: Store,
    }

    impl Machine {
        fn new(id: u128) -> Machine {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::under(dir.path());
            let device = Device {
                device_id: Uuid::from_u128(id),
                active_plan: None,
            };
            Machine {
                _dir: dir,
                paths,
                device,
                store: Store::default(),
            }
        }

        /// A machine with a saved store holding the starter week.
        fn with_plans(id: u128) -> Machine {
            let mut m = Machine::new(id);
            m.store.plans = vec![starter_plan("Base")];
            storage::save(&m.paths, &mut m.store).unwrap();
            m
        }

        /// Edit and save, as the TUI's `commit` does.
        fn edit(&mut self, f: impl FnOnce(&mut Store)) {
            f(&mut self.store);
            storage::save(&self.paths, &mut self.store).unwrap();
        }

        async fn sync(&mut self, fake: &Fake) -> Result<Report> {
            sync(&self.paths, fake, &mut self.store, &self.device).await
        }

        fn on_disk(&self) -> Store {
            storage::load(&self.paths).unwrap().0
        }

        fn conflict_files(&self) -> Vec<PathBuf> {
            let Ok(dir) = fs::read_dir(self.paths.sync_dir()) else {
                return Vec::new();
            };
            let mut v: Vec<_> = dir
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("conflict-")
                })
                .collect();
            v.sort();
            v
        }
    }

    fn notes(s: &Store, day: usize, k: usize) -> &str {
        &s.plans[0].days[day][k].notes
    }

    fn set_notes(day: usize, k: usize, text: &str) -> impl FnOnce(&mut Store) {
        move |s| text.clone_into(&mut s.plans[0].days[day][k].notes)
    }

    /// Two machines sharing one fake Drive, both synced to the same document.
    async fn pair() -> (Fake, Machine, Machine) {
        let fake = Fake::default();
        let mut a = Machine::with_plans(1);
        let mut b = Machine::new(2);
        assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::Created);
        assert_eq!(b.sync(&fake).await.unwrap().outcome, Outcome::Pulled);
        assert_eq!(b.store, a.store);
        (fake, a, b)
    }

    #[tokio::test]
    async fn create_then_up_to_date() {
        let fake = Fake::default();
        let mut a = Machine::with_plans(1);
        let r = a.sync(&fake).await.unwrap();
        assert_eq!(r.outcome, Outcome::Created);
        assert_eq!(fake.head(0), a.store);
        let state = read_state(&a.paths).unwrap().unwrap();
        assert_eq!(
            state.base_head,
            fake.files.lock().unwrap()[0].meta.head_revision_id
        );
        assert!(!is_dirty(&a.paths, &a.store).unwrap());

        let r = a.sync(&fake).await.unwrap();
        assert_eq!(r.outcome, Outcome::UpToDate);
        assert_eq!(fake.revs(0), 1);
        assert!(r.conflict_files.is_empty() && r.warnings.is_empty());
    }

    /// A fresh machine takes Drive's store instead of merging its unsaved default in.
    #[tokio::test]
    async fn fresh_machine_takes_drive() {
        let (fake, a, b) = pair().await;
        assert_eq!(b.on_disk(), a.store);
        assert_eq!(b.store.plans.len(), 1);
        assert!(b.conflict_files().is_empty());
        assert_eq!(fake.revs(0), 1);
    }

    #[tokio::test]
    async fn local_change_pushes() {
        let (fake, mut a, _) = pair().await;
        a.edit(set_notes(0, 0, "pushed"));
        assert!(is_dirty(&a.paths, &a.store).unwrap());
        assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::Pushed);
        assert_eq!(notes(&fake.head(0), 0, 0), "pushed");
        assert!(!is_dirty(&a.paths, &a.store).unwrap());
    }

    #[tokio::test]
    async fn remote_change_over_clean_local_pulls() {
        let (fake, mut a, mut b) = pair().await;
        a.edit(set_notes(0, 0, "from a"));
        a.sync(&fake).await.unwrap();
        let r = b.sync(&fake).await.unwrap();
        assert_eq!(r.outcome, Outcome::Pulled);
        assert_eq!(b.store, a.store);
        assert_eq!(b.on_disk(), a.store);
        assert!(r.conflict_files.is_empty());
        assert_eq!(b.sync(&fake).await.unwrap().outcome, Outcome::UpToDate);
    }

    #[tokio::test]
    async fn disjoint_edits_merge_without_a_conflict_file() {
        let (fake, mut a, mut b) = pair().await;
        a.edit(set_notes(0, 0, "from a"));
        a.sync(&fake).await.unwrap();
        b.edit(set_notes(1, 0, "from b"));

        let r = b.sync(&fake).await.unwrap();
        assert_eq!(r.outcome, Outcome::Merged { conflicts: vec![] });
        for s in [&b.store, &b.on_disk(), &fake.head(0)] {
            assert_eq!((notes(s, 0, 0), notes(s, 1, 0)), ("from a", "from b"));
        }
        assert!(r.conflict_files.is_empty());
        assert!(b.conflict_files().is_empty());

        // The other machine then just pulls the merge.
        assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::Pulled);
        assert_eq!(a.store, b.store);
    }

    /// The later edit wins; the local store that lost it is saved first.
    #[tokio::test]
    async fn same_edit_on_both_sides_saves_the_losing_local() {
        let (fake, mut a, mut b) = pair().await;
        b.edit(set_notes(0, 0, "from b"));
        let before = b.store.clone();
        a.edit(set_notes(0, 0, "from a"));
        a.sync(&fake).await.unwrap();

        let r = b.sync(&fake).await.unwrap();
        let Outcome::Merged { conflicts } = &r.outcome else {
            panic!("{:?}", r.outcome)
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(notes(&b.store, 0, 0), "from a");
        assert_eq!(notes(&fake.head(0), 0, 0), "from a");
        assert_eq!(r.conflict_files, b.conflict_files());
        let [saved] = &r.conflict_files[..] else {
            panic!("{:?}", r.conflict_files)
        };
        assert_eq!(storage::parse(&fs::read(saved).unwrap()).unwrap(), before);
        let text = describe(&conflicts[0], &b.store);
        assert!(text.starts_with("Mon "), "{text}");
        assert!(text.contains("in \"Base\": kept"), "{text}");
    }

    #[tokio::test]
    async fn other_remote_schema_is_never_written() {
        let (fake, mut a, _) = pair().await;
        a.edit(set_notes(0, 0, "local"));
        let state = read_state(&a.paths).unwrap();
        for schema in [Some(STORE_VERSION - 1), Some(STORE_VERSION + 1), None] {
            fake.files.lock().unwrap()[0].meta.schema = schema;
            assert!(a.sync(&fake).await.is_err());
            let kept = keep(&a.paths, &fake, &mut a.store, &a.device, Side::Local).await;
            assert!(kept.is_err());
            assert_eq!(fake.revs(0), 1);
            assert_eq!(read_state(&a.paths).unwrap(), state);
            assert_eq!(notes(&a.on_disk(), 0, 0), "local");
        }
    }

    /// Another device writes `text` on the first session of `day`, stamped `at`.
    fn race(day: usize, text: &str, at: DateTime<Utc>) -> Race {
        let text = text.to_owned();
        Box::new(move |s| {
            let it = &mut s.plans[0].days[day][0];
            it.notes = text;
            it.updated_at = at;
        })
    }

    /// Another upload lands between our head check and our update: ours replaces it, so the
    /// check after the write finds it in the revision list, merges it and uploads again.
    #[tokio::test]
    async fn verify_after_write_merges_a_racing_upload() {
        let (fake, mut a, mut b) = pair().await;
        fake.races
            .lock()
            .unwrap()
            .push(race(2, "raced", Utc::now()));
        b.edit(set_notes(1, 0, "from b"));

        let r = b.sync(&fake).await.unwrap();
        assert_eq!(r.outcome, Outcome::Merged { conflicts: vec![] });
        // The base, the race, b's first upload, the merged one.
        assert_eq!(fake.revs(0), 4);
        for s in [&b.store, &b.on_disk(), &fake.head(0)] {
            assert_eq!((notes(s, 1, 0), notes(s, 2, 0)), ("from b", "raced"));
        }
        assert_eq!(b.sync(&fake).await.unwrap().outcome, Outcome::UpToDate);
        assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::Pulled);
        assert_eq!(a.store, b.store);
    }

    #[tokio::test]
    async fn verify_after_write_gives_up_after_bounded_retries() {
        let (fake, _, mut b) = pair().await;
        for i in 0..MAX_UPLOADS {
            let at = DateTime::from_timestamp(100 + i64::try_from(i).unwrap(), 0).unwrap();
            fake.races
                .lock()
                .unwrap()
                .push(race(3, &format!("race {i}"), at));
        }
        b.edit(set_notes(1, 0, "from b"));
        let err = b.sync(&fake).await.unwrap_err().to_string();
        assert!(err.contains("kept changing"), "{err}");
        assert_eq!(fake.revs(0), 1 + 2 * MAX_UPLOADS);
        // Nothing is lost: the last race is merged here, and b is dirty for the next sync.
        let last = format!("race {}", MAX_UPLOADS - 1);
        let disk = b.on_disk();
        assert_eq!((notes(&disk, 1, 0), notes(&disk, 3, 0)), ("from b", &*last));
        assert!(is_dirty(&b.paths, &b.store).unwrap());
        assert_eq!(b.sync(&fake).await.unwrap().outcome, Outcome::Pushed);
        assert_eq!(notes(&fake.head(0), 3, 0), last);
    }

    #[test]
    fn written_between_needs_both_revisions_in_order() {
        let rev = |id: &str, t: Option<i64>| RemoteRevision {
            id: id.into(),
            modified: t.map(|t| DateTime::from_timestamp(t, 0).unwrap()),
        };
        // Listed out of order: sorted by time.
        let revs = [
            rev("b", Some(1)),
            rev("x", Some(2)),
            rev("o", Some(3)),
            rev("a", Some(0)),
        ];
        assert_eq!(written_between(&revs, "b", "o").unwrap(), ["x"]);
        assert_eq!(
            written_between(&revs, "a", "b").unwrap(),
            Vec::<String>::new()
        );
        // Without every time, list order; missing or reversed revisions cannot be checked.
        let revs = [rev("b", None), rev("x", Some(2)), rev("o", Some(3))];
        assert_eq!(written_between(&revs, "b", "o").unwrap(), ["x"]);
        assert_eq!(written_between(&revs, "gone", "o"), None);
        assert_eq!(written_between(&revs, "o", "b"), None);
    }

    /// Two first syncs raced and made two files: the oldest wins, the other is merged in and
    /// deleted, and the machine that made it follows the oldest from then on.
    #[tokio::test]
    async fn several_remote_files_merge_into_the_oldest() {
        let fake = Fake::default();
        let mut a = Machine::with_plans(1);
        let mut b = Machine::with_plans(2);
        b.edit(|s| s.plans[0].name = "B's".into());
        a.sync(&fake).await.unwrap();
        // b's first sync did not see a's file.
        let files = std::mem::take(&mut *fake.files.lock().unwrap());
        b.sync(&fake).await.unwrap();
        fake.files.lock().unwrap().splice(0..0, files);
        assert_eq!(fake.count(), 2);
        b.edit(set_notes(0, 0, "from b"));

        let r = b.sync(&fake).await.unwrap();
        assert!(
            matches!(r.outcome, Outcome::Merged { .. }),
            "{:?}",
            r.outcome
        );
        assert!(r.warnings[0].contains("2 copies"), "{:?}", r.warnings);
        assert_eq!(fake.count(), 1);
        let head = fake.head(0);
        assert_eq!(head, b.store);
        // No common base: both plans count as new, b's first.
        assert_eq!(names(&head), ["B's", "Base"]);
        assert_eq!(notes(&head, 0, 0), "from b");
        assert_eq!(head.plans[1], a.store.plans[0]);
        let state = read_state(&b.paths).unwrap().unwrap();
        assert_eq!(state.file_id, fake.files.lock().unwrap()[0].meta.file_id);

        assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::Pulled);
        assert_eq!(a.store, b.store);
    }

    fn names(s: &Store) -> Vec<&str> {
        s.plans.iter().map(|p| p.name.as_str()).collect()
    }

    #[tokio::test]
    async fn keep_overwrites_one_side_and_saves_the_other() {
        let (fake, mut a, mut b) = pair().await;
        a.edit(set_notes(0, 0, "from a"));
        a.sync(&fake).await.unwrap();
        b.edit(set_notes(0, 0, "from b"));
        let drive = fake.head(0);

        let r = keep(&b.paths, &fake, &mut b.store, &b.device, Side::Local)
            .await
            .unwrap();
        assert_eq!(r.outcome, Outcome::Kept(Side::Local));
        assert_eq!(notes(&fake.head(0), 0, 0), "from b");
        let [saved] = &r.conflict_files[..] else {
            panic!()
        };
        assert_eq!(storage::parse(&fs::read(saved).unwrap()).unwrap(), drive);

        a.edit(set_notes(0, 0, "a again"));
        let mine = a.store.clone();
        let r = keep(&a.paths, &fake, &mut a.store, &a.device, Side::Remote)
            .await
            .unwrap();
        assert_eq!(notes(&a.store, 0, 0), "from b");
        assert_eq!(a.on_disk(), a.store);
        assert_eq!(
            storage::parse(&fs::read(&r.conflict_files[0]).unwrap()).unwrap(),
            mine
        );
        assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::UpToDate);

        let empty = Fake::default();
        assert!(
            keep(&a.paths, &empty, &mut a.store, &a.device, Side::Remote)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn several_rounds_of_alternating_edits_converge() {
        let (fake, mut a, mut b) = pair().await;
        for round in 0..3usize {
            let (da, db) = (round * 2, round * 2 + 1);
            a.edit(set_notes(da, 0, &format!("a{round}")));
            a.sync(&fake).await.unwrap();
            b.edit(set_notes(db, 0, &format!("b{round}")));
            let r = b.sync(&fake).await.unwrap();
            assert!(
                matches!(r.outcome, Outcome::Merged { .. }),
                "{:?}",
                r.outcome
            );
            assert_eq!(a.sync(&fake).await.unwrap().outcome, Outcome::Pulled);
        }
        assert_eq!(a.store, b.store);
        assert_eq!(a.on_disk(), b.store);
        assert_eq!(b.on_disk(), b.store);
        assert_eq!(fake.head(0), a.store);
        for round in 0..3usize {
            let (da, db) = (round * 2, round * 2 + 1);
            assert_eq!(notes(&a.store, da, 0), format!("a{round}"));
            assert_eq!(notes(&a.store, db, 0), format!("b{round}"));
        }
    }

    #[tokio::test]
    async fn first_sync_with_nothing_on_drive_creates_from_local() {
        let fake = Fake::default();
        let mut a = Machine::new(1);
        let r = a.sync(&fake).await.unwrap();
        assert_eq!(r.outcome, Outcome::Created);
        assert_eq!(fake.head(0), a.store);
    }

    #[tokio::test]
    async fn remote_status_writes_nothing() {
        let fake = Fake::default();
        let mut a = Machine::with_plans(1);
        assert_eq!(
            remote_status(&a.paths, &fake).await.unwrap().0,
            Drift::Missing
        );
        a.sync(&fake).await.unwrap();
        assert_eq!(
            remote_status(&a.paths, &fake).await.unwrap().0,
            Drift::Unchanged
        );
        fake.upload(0, &a.store);
        let state = read_state(&a.paths).unwrap();
        assert_eq!(
            remote_status(&a.paths, &fake).await.unwrap().0,
            Drift::Changed
        );
        assert_eq!(read_state(&a.paths).unwrap(), state);
        let b = Machine::new(2);
        assert_eq!(
            remote_status(&b.paths, &fake).await.unwrap().0,
            Drift::Untracked
        );
    }
}
