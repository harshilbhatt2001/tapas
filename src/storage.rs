//! Where tapas keeps its files, and loading and saving the store.

use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use directories::ProjectDirs;
use serde::Deserialize;

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

#[derive(Deserialize)]
struct Version {
    version: u32,
}

/// Parse a store document of exactly [`STORE_VERSION`], without `normalize`. The version is
/// checked first because serde would silently drop a newer version's fields.
pub fn parse(bytes: &[u8]) -> Result<Store> {
    let Version { version } = serde_json::from_slice(bytes)?;
    if version != STORE_VERSION {
        bail!("store version {version}, but this tapas reads only version {STORE_VERSION}");
    }
    Ok(serde_json::from_slice(bytes)?)
}

/// The store `file` holds, to diff the next save against; `None` when there is none.
fn previous(file: &Path) -> Result<Option<Store>> {
    match fs::read(file) {
        Ok(bytes) => parse(&bytes)
            .map(Some)
            .with_context(|| format!("not overwriting {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// The saved store, or `Store::default()` when there is none yet; and this machine's device
/// state, created and saved on first load.
pub fn load(paths: &Paths) -> Result<(Store, Device)> {
    let file = &paths.store_file;
    let mut store = match fs::read(file) {
        Ok(bytes) => parse(&bytes).with_context(|| format!("reading {}", file.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    store.normalize();

    let device_file = paths.device_file();
    let device = match fs::read(&device_file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("reading {}", device_file.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let device = Device::default();
            save_device(paths, &device)?;
            device
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", device_file.display())),
    };
    Ok((store, device))
}

/// Save the store, unless the file on disk is not a store of this version. First stamps
/// `updated_at` on what changed since the file on disk (see [`merge::stamp`]), in `store` too,
/// so edit sites need not. Without a previous file nothing is stamped.
pub fn save(paths: &Paths, store: &mut Store) -> Result<()> {
    if let Some(prev) = previous(&paths.store_file)? {
        merge::stamp(&prev, store, Utc::now());
    }
    write_atomic(&paths.store_file, &serde_json::to_vec_pretty(store)?)
}

/// Save a store as it is, without stamping: for sync results, whose stamps come from the
/// merge. Refuses to overwrite what [`save`] refuses to.
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

    #[test]
    fn corrupt_store_is_neither_loaded_nor_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        write_atomic(&paths.store_file, b"{ not json").unwrap();
        assert!(load(&paths).is_err());
        assert!(save(&paths, &mut Store::default()).is_err());
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

    /// A store of another version, older or newer, is neither loaded nor overwritten.
    #[test]
    fn other_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        for version in [STORE_VERSION - 1, STORE_VERSION + 1] {
            let mut doc = serde_json::to_value(Store::default()).unwrap();
            doc["version"] = version.into();
            let other = serde_json::to_vec(&doc).unwrap();
            write_atomic(&paths.store_file, &other).unwrap();

            assert!(load(&paths).is_err());
            assert!(save(&paths, &mut Store::default()).is_err());
            assert!(write_store(&paths, &Store::default()).is_err());
            assert_eq!(fs::read(&paths.store_file).unwrap(), other);
        }
    }
}
