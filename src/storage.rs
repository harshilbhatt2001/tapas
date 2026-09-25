//! Where tapas keeps its files, and loading, migrating and saving the store.

use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use directories::ProjectDirs;
use serde_json::Value;

use crate::{
    model::{Device, STORE_VERSION, Store},
    sync::merge,
};

/// Data and config directories: the platform's project dirs, or `$TAPAS_HOME/{data,config}`.
/// `$TAPAS_STORE` moves only the store file, for example into a synced folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub store_file: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Paths> {
        Paths::from_env(
            std::env::var_os("TAPAS_HOME"),
            std::env::var_os("TAPAS_STORE"),
        )
    }

    /// Store file precedence: `tapas_store` > `<tapas_home>/data/store.json` > platform data dir.
    pub fn from_env(tapas_home: Option<OsString>, tapas_store: Option<OsString>) -> Result<Paths> {
        let mut paths = if let Some(home) = tapas_home.filter(|h| !h.is_empty()) {
            Paths::under(Path::new(&home))
        } else {
            let dirs = ProjectDirs::from("", "", "tapas").context("no home directory found")?;
            let data_dir = dirs.data_dir().to_path_buf();
            Paths {
                store_file: data_dir.join("store.json"),
                data_dir,
                config_dir: dirs.config_dir().to_path_buf(),
            }
        };
        if let Some(file) = tapas_store.filter(|f| !f.is_empty()) {
            paths.store_file = PathBuf::from(file);
        }
        Ok(paths)
    }

    /// `<home>/data` and `<home>/config`.
    #[must_use]
    pub fn under(home: &Path) -> Paths {
        let data_dir = home.join("data");
        Paths {
            store_file: data_dir.join("store.json"),
            data_dir,
            config_dir: home.join("config"),
        }
    }

    /// Device-local state; stays in the data dir when `TAPAS_STORE` moves the store.
    #[must_use]
    pub fn device_file(&self) -> PathBuf {
        self.data_dir.join("device.json")
    }

    /// Google "Desktop app" OAuth client JSON.
    #[must_use]
    pub fn client_secret_file(&self) -> PathBuf {
        self.config_dir.join("client_secret.json")
    }

    /// OAuth token cache of one Google API (`calendar`, `health`).
    #[must_use]
    pub fn tokens_file(&self, api: &str) -> PathBuf {
        self.config_dir.join(format!("tokens-{api}.json"))
    }

    /// Pre-split cache that held one token for every scope; the Health API rejects it.
    #[must_use]
    pub fn legacy_tokens_file(&self) -> PathBuf {
        self.config_dir.join("tokens.json")
    }

    /// Drive sync bookkeeping (`state.json`, `base.json`, `conflict-*.json`); never synced.
    #[must_use]
    pub fn sync_dir(&self) -> PathBuf {
        self.data_dir.join("sync")
    }
}

/// Write to a temporary file next to `path`, then rename it over the old one, so readers and
/// folder-sync tools never see a half-written file. Creates missing parent directories.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // `.store.json.XXXXXX.tmp`: hidden, and clearly ours if a crash leaves it behind.
    let mut prefix = OsString::from(".");
    prefix.push(path.file_name().unwrap_or_default());
    prefix.push(".");
    let mut tmp = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("creating a temporary file in {}", dir.display()))?;
    tmp.write_all(bytes)
        .with_context(|| format!("writing {}", tmp.path().display()))?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Device-local state that [`migrate`] moved out of an older store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub active_plan: Option<String>,
}

fn version_of(doc: &Value) -> Result<u32> {
    doc.get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .context("no store version")
}

/// Upgrade a raw store document to [`STORE_VERSION`], one version step at a time, before it
/// is parsed into a [`Store`]. A newer version is an error: serde would drop its new fields.
pub fn migrate(mut doc: Value) -> Result<(Value, Moved)> {
    let mut moved = Moved::default();
    loop {
        match version_of(&doc)? {
            STORE_VERSION => return Ok((doc, moved)),
            v if v > STORE_VERSION => bail!(
                "store version {v} is newer than this tapas supports ({STORE_VERSION}); update tapas"
            ),
            1 => moved.active_plan = v1_to_v2(&mut doc)?,
            2 => v2_to_v3(&mut doc)?,
            v => bail!("unknown store version {v}"),
        }
    }
}

/// v2 drops the `active` plan index; it becomes [`Device::active_plan`], a plan id.
fn v1_to_v2(doc: &mut Value) -> Result<Option<String>> {
    let obj = doc.as_object_mut().context("store is not a JSON object")?;
    let active = obj
        .remove("active")
        .and_then(|a| a.as_u64())
        .and_then(|a| usize::try_from(a).ok())
        .unwrap_or(0);
    obj.insert("version".into(), 2.into());
    let plans = obj.get("plans").and_then(Value::as_array);
    // v1 clamped an out-of-range index to the last plan.
    let plan = plans.and_then(|p| p.get(active).or_else(|| p.last()));
    Ok(plan
        .and_then(|p| p.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned))
}

/// v3 adds `updated_at` to plans, items, session types, profile and export settings. They
/// default to the Unix epoch ("never edited"), so only the version moves.
fn v2_to_v3(doc: &mut Value) -> Result<()> {
    let obj = doc.as_object_mut().context("store is not a JSON object")?;
    obj.insert("version".into(), 3.into());
    Ok(())
}

/// The store `file` holds, to diff the next save against; `None` when there is none or it
/// does not parse. Fails if the file has a newer version than this binary knows.
fn previous(file: &Path) -> Result<Option<Store>> {
    let bytes = match fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    if let Ok(v) = version_of(&doc)
        && v > STORE_VERSION
    {
        bail!(
            "not overwriting {}: store version {v} is newer than this tapas supports ({STORE_VERSION})",
            file.display()
        );
    }
    Ok(migrate(doc)
        .ok()
        .and_then(|(doc, _)| serde_json::from_value(doc).ok()))
}

/// Parse a store document of any known version into a [`Store`], without `normalize`.
pub fn parse(bytes: &[u8]) -> Result<Store> {
    let (doc, _) = migrate(serde_json::from_slice(bytes)?)?;
    Ok(serde_json::from_value(doc)?)
}

/// The saved store, migrated to the current version, or `Store::default()` when there is none
/// yet; and this machine's device state. A new or migrated device state is saved right away.
pub fn load(paths: &Paths) -> Result<(Store, Device)> {
    let file = &paths.store_file;
    let (mut store, moved) = match fs::read(file) {
        Ok(bytes) => {
            let parse = || -> Result<(Store, Moved)> {
                let (doc, moved) = migrate(serde_json::from_slice(&bytes)?)?;
                Ok((serde_json::from_value(doc)?, moved))
            };
            parse().with_context(|| format!("reading {}", file.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Store::default(), Moved::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    store.normalize();

    let device_file = paths.device_file();
    let (mut device, mut changed) = match fs::read(&device_file) {
        Ok(bytes) => {
            let device: Device = serde_json::from_slice(&bytes)
                .with_context(|| format!("reading {}", device_file.display()))?;
            (device, false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Device::default(), true),
        Err(e) => return Err(e).with_context(|| format!("reading {}", device_file.display())),
    };
    if device.active_plan.is_none() && moved.active_plan.is_some() {
        device.active_plan = moved.active_plan;
        changed = true;
    }
    if changed {
        save_device(paths, &device)?;
    }
    Ok((store, device))
}

/// Save the store, unless the file on disk has a newer version (written by a newer tapas).
/// First stamps `updated_at` on what changed since the file on disk (see [`merge::stamp`]),
/// in `store` too, so edit sites need not. Without a readable previous file nothing is stamped.
pub fn save(paths: &Paths, store: &mut Store) -> Result<()> {
    if let Some(prev) = previous(&paths.store_file)? {
        merge::stamp(&prev, store, Utc::now());
    }
    write_atomic(&paths.store_file, &serde_json::to_vec_pretty(store)?)
}

/// Save a store as it is, without stamping: for sync results, whose stamps come from the
/// merge. Like [`save`], refuses to overwrite a newer version.
pub fn write_store(paths: &Paths, store: &Store) -> Result<()> {
    previous(&paths.store_file)?;
    write_atomic(&paths.store_file, &serde_json::to_vec_pretty(store)?)
}

pub fn save_device(paths: &Paths, device: &Device) -> Result<()> {
    write_atomic(&paths.device_file(), &serde_json::to_vec_pretty(device)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::starter_plan;
    use chrono::DateTime;

    /// `TAPAS_HOME` overrides the platform project dirs with `<home>/{data,config}`.
    #[test]
    fn tapas_home_overrides_platform_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: no other test in this process reads or writes TAPAS_HOME.
        unsafe {
            std::env::set_var("TAPAS_HOME", dir.path());
        }
        let paths = Paths::resolve().unwrap();
        unsafe {
            std::env::remove_var("TAPAS_HOME");
        }
        assert_eq!(paths.data_dir, dir.path().join("data"));
        assert_eq!(paths.config_dir, dir.path().join("config"));
    }

    /// `TAPAS_STORE` > `$TAPAS_HOME/data/store.json` > platform data dir, and only the store
    /// file moves.
    #[test]
    fn tapas_store_precedence() {
        let home = Path::new("/h");
        let store = || Some(OsString::from("/sync/tapas.json"));

        let p = Paths::from_env(Some(home.into()), store()).unwrap();
        assert_eq!(p.store_file, Path::new("/sync/tapas.json"));
        assert_eq!(p.data_dir, home.join("data"));
        assert_eq!(p.config_dir, home.join("config"));
        assert_eq!(p.device_file(), home.join("data/device.json"));

        let p = Paths::from_env(Some(home.into()), None).unwrap();
        assert_eq!(p.store_file, home.join("data/store.json"));

        let p = Paths::from_env(None, store()).unwrap();
        assert_eq!(p.store_file, Path::new("/sync/tapas.json"));
        assert_ne!(p.data_dir, Path::new("/sync"));

        let p = Paths::from_env(None, None).unwrap();
        assert_eq!(p.store_file, p.data_dir.join("store.json"));

        // Empty values count as unset.
        let p = Paths::from_env(Some(OsString::new()), Some(OsString::new())).unwrap();
        assert_eq!(p, Paths::from_env(None, None).unwrap());
    }

    /// A corrupt store file is reported as an error, not silently replaced.
    #[test]
    fn corrupt_json_is_an_error_not_a_silent_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        fs::create_dir_all(&paths.data_dir).unwrap();
        fs::write(&paths.store_file, b"{ not json").unwrap();
        let err = load(&paths).unwrap_err();
        assert!(err.to_string().contains("reading"));
        // The corrupt file must still be there, untouched.
        assert_eq!(fs::read_to_string(&paths.store_file).unwrap(), "{ not json");
    }

    /// A fresh device gets a stable id: the device file is written on first load.
    #[test]
    fn missing_file_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (s, device) = load(&paths).unwrap();
        assert_eq!(s.plans.len(), 1);
        assert_eq!(s.plans[0].name, "Week 1");
        assert!(s.plans[0].is_empty());
        assert_eq!(device.active_plan, None);
        assert_eq!(load(&paths).unwrap().1, device);
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut s = Store::default();
        s.plans.push(starter_plan("Base"));
        s.profile.weight = 70.5;
        save(&paths, &mut s).unwrap();
        let device = Device {
            active_plan: Some(s.plans[1].id.clone()),
            ..Device::default()
        };
        save_device(&paths, &device).unwrap();
        assert_eq!(load(&paths).unwrap(), (s, device));

        let raw = fs::read_to_string(&paths.store_file).unwrap();
        assert!(raw.contains(r#""start": "07:30""#));
        assert!(raw.contains(r#""version": 3"#));
    }

    /// The target is replaced whole, parents are created and no temporary file is left.
    #[test]
    fn write_atomic_replaces_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a/b/store.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
        let names: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["store.json"]);
    }

    /// A v1 store as the previous release wrote it: `active` is an index into `plans`.
    fn v1_store(active: usize) -> (Store, Vec<u8>) {
        let mut s = Store::default();
        s.plans.push(starter_plan("Base"));
        s.plans.push(starter_plan("Build"));
        let mut doc = serde_json::to_value(&s).unwrap();
        doc["version"] = 1.into();
        doc["active"] = active.into();
        (s, serde_json::to_vec_pretty(&doc).unwrap())
    }

    #[test]
    fn v1_active_index_becomes_device_plan_id() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (s, v1) = v1_store(1);
        write_atomic(&paths.store_file, &v1).unwrap();

        let (mut store, device) = load(&paths).unwrap();
        assert_eq!(store, s);
        assert_eq!(store.version, STORE_VERSION);
        assert_eq!(device.active_plan.as_deref(), Some(s.plans[1].id.as_str()));
        let saved: Device =
            serde_json::from_slice(&fs::read(paths.device_file()).unwrap()).unwrap();
        assert_eq!(saved, device);

        // Saving writes the current version without `active`; loading again keeps the device.
        save(&paths, &mut store).unwrap();
        let raw = fs::read_to_string(&paths.store_file).unwrap();
        assert!(!raw.contains(r#""active""#));
        assert_eq!(load(&paths).unwrap().1, device);
    }

    #[test]
    fn v1_out_of_range_index_picks_the_last_plan() {
        let (s, v1) = v1_store(9);
        let (doc, moved) = migrate(serde_json::from_slice(&v1).unwrap()).unwrap();
        assert_eq!(doc["version"], STORE_VERSION);
        assert!(doc.get("active").is_none());
        assert_eq!(moved.active_plan.as_deref(), Some(s.plans[2].id.as_str()));
    }

    /// A v2 store has no `updated_at` anywhere: it loads with every stamp at the epoch.
    #[test]
    fn v2_gains_epoch_stamps() {
        let mut s = Store::default();
        s.plans.push(starter_plan("Base"));
        let mut doc = serde_json::to_value(&s).unwrap();
        doc["version"] = 2.into();
        let strip = |v: &mut Value| {
            v.as_object_mut().unwrap().remove("updated_at");
        };
        strip(&mut doc["profile"]);
        strip(&mut doc["export"]);
        doc["library"]["types"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .for_each(strip);
        for plan in doc["plans"].as_array_mut().unwrap() {
            strip(plan);
            for day in plan["days"].as_array_mut().unwrap() {
                day.as_array_mut().unwrap().iter_mut().for_each(strip);
            }
        }
        assert!(!doc.to_string().contains("updated_at"));

        let (doc, moved) = migrate(doc).unwrap();
        assert_eq!(doc["version"], 3);
        assert_eq!(moved, Moved::default());
        let store: Store = serde_json::from_value(doc).unwrap();
        assert_eq!(store, s);
        assert_eq!(store.plans[1].days[0][0].updated_at, DateTime::UNIX_EPOCH);
    }

    /// `save` stamps what changed since the file on disk, in memory and on disk alike.
    #[test]
    fn save_stamps_only_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut s = Store::default();
        s.plans.push(starter_plan("Base"));
        save(&paths, &mut s).unwrap();
        let before = s.clone();

        s.plans[1].days[2][0].notes = "edited".into();
        save(&paths, &mut s).unwrap();
        assert!(s.plans[1].days[2][0].updated_at > DateTime::UNIX_EPOCH);
        let mut unstamped = s.clone();
        unstamped.plans[1].days[2][0].updated_at = DateTime::UNIX_EPOCH;
        unstamped.plans[1].days[2][0].notes.clear();
        assert_eq!(unstamped, before);
        assert_eq!(load(&paths).unwrap().0, s);
    }

    /// A store from a newer tapas is neither loaded nor overwritten.
    #[test]
    fn newer_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut doc = serde_json::to_value(Store::default()).unwrap();
        doc["version"] = (STORE_VERSION + 1).into();
        doc["future"] = "field".into();
        let newer = serde_json::to_vec(&doc).unwrap();
        write_atomic(&paths.store_file, &newer).unwrap();

        let err = format!("{:#}", load(&paths).unwrap_err());
        assert!(err.contains("newer than this tapas supports"), "{err}");
        let err = format!("{:#}", save(&paths, &mut Store::default()).unwrap_err());
        assert!(err.contains("not overwriting"), "{err}");
        assert_eq!(fs::read(&paths.store_file).unwrap(), newer);
    }
}
