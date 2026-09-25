//! Where tapas keeps its files, and loading and saving the store.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;

use crate::model::Store;

/// Data and config directories: the platform's project dirs, or `$TAPAS_HOME/{data,config}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Paths> {
        if let Some(home) = std::env::var_os("TAPAS_HOME") {
            return Ok(Paths::under(Path::new(&home)));
        }
        let dirs = ProjectDirs::from("", "", "tapas").context("no home directory found")?;
        Ok(Paths {
            data_dir: dirs.data_dir().to_path_buf(),
            config_dir: dirs.config_dir().to_path_buf(),
        })
    }

    /// `<home>/data` and `<home>/config`.
    #[must_use]
    pub fn under(home: &Path) -> Paths {
        Paths {
            data_dir: home.join("data"),
            config_dir: home.join("config"),
        }
    }

    #[must_use]
    pub fn store_file(&self) -> PathBuf {
        self.data_dir.join("store.json")
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
}

/// The saved store, or `Store::default()` when there is none yet.
pub fn load(paths: &Paths) -> Result<Store> {
    let file = paths.store_file();
    let mut store: Store = match fs::read(&file) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("reading {}", file.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    store.normalize();
    Ok(store)
}

/// Write to a temporary file next to the store, then rename it over the old one.
pub fn save(paths: &Paths, store: &Store) -> Result<()> {
    let file = paths.store_file();
    fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("creating {}", paths.data_dir.display()))?;
    let tmp = file.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(store)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &file).with_context(|| format!("replacing {}", file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::starter_plan;

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

    /// A corrupt store file is reported as an error, not silently replaced.
    #[test]
    fn corrupt_json_is_an_error_not_a_silent_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        fs::create_dir_all(&paths.data_dir).unwrap();
        fs::write(paths.store_file(), b"{ not json").unwrap();
        let err = load(&paths).unwrap_err();
        assert!(err.to_string().contains("reading"));
        // The corrupt file must still be there, untouched.
        assert_eq!(
            fs::read_to_string(paths.store_file()).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn missing_file_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        let s = load(&Paths::under(dir.path())).unwrap();
        assert_eq!(s.plans.len(), 1);
        assert_eq!(s.plans[0].name, "Week 1");
        assert!(s.plans[0].is_empty());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut s = Store::default();
        s.plans.push(starter_plan("Base"));
        s.active = 1;
        s.profile.weight = 70.5;
        save(&paths, &s).unwrap();
        assert!(!paths.store_file().with_extension("json.tmp").exists());
        assert_eq!(load(&paths).unwrap(), s);

        let raw = fs::read_to_string(paths.store_file()).unwrap();
        assert!(raw.contains(r#""start": "07:30""#));
    }
}
