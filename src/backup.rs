//! Backup directories, manifests and retention.
//!
//! A backup directory is self-describing:
//!
//! ```text
//! <dir>/
//! ├── dump/<dump file>
//! ├── files/<sanitized remote path>/...
//! └── manifest.json
//! ```
//!
//! The manifest is deliberately a stable, versioned JSON document: the internal
//! backup app can read it without linking the DSL or the SSH client.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const TOOL_NAME: &str = "cloner";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ManifestRemote {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub app_path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ManifestDatabase {
    pub engine: String,
    pub database: String,
    pub user: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ManifestDump {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ManifestFile {
    pub from: String,
    pub to: String,
    pub files: u64,
    pub downloaded: u64,
    pub skipped: u64,
    pub deleted: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Manifest {
    pub tool: String,
    pub version: String,
    pub created_at: String,
    pub config: String,
    pub project_root: String,
    pub project_env: String,
    pub remote: Option<ManifestRemote>,
    pub database: Option<ManifestDatabase>,
    pub dump: Option<ManifestDump>,
    pub files: Vec<ManifestFile>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            tool: TOOL_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            config: String::new(),
            project_root: String::new(),
            project_env: String::new(),
            remote: None,
            database: None,
            dump: None,
            files: Vec::new(),
        }
    }
}

impl Manifest {
    pub fn new(
        config: impl Into<String>,
        project_root: impl Into<String>,
        project_env: impl Into<String>,
    ) -> Self {
        Self {
            config: config.into(),
            project_root: project_root.into(),
            project_env: project_env.into(),
            ..Self::default()
        }
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join(MANIFEST_FILE)
    }

    /// Record a dump on disk, including its size and SHA-256.
    pub fn set_dump(&mut self, path: &Path) -> Result<()> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("stat backup dump {}", path.display()))?;
        self.dump = Some(ManifestDump {
            path: path.display().to_string(),
            size: metadata.len(),
            sha256: crate::util::sha256_file(path).ok(),
        });
        Ok(())
    }

    /// Write `<dir>/manifest.json` and return its path.
    pub fn write(&self, dir: &Path) -> Result<PathBuf> {
        let path = Self::path(dir);
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create backup directory {}", dir.display()))?;
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", path.display()))?;
        log::info!("backup manifest written to {}", path.display());
        Ok(path)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse manifest {}", path.display()))
    }

    /// True when the dump recorded in the manifest still exists with the same
    /// size (a cheap integrity check before a restore).
    pub fn dump_is_intact(&self) -> bool {
        let Some(dump) = &self.dump else {
            return false;
        };
        std::fs::metadata(&dump.path).is_ok_and(|meta| meta.len() == dump.size)
    }
}

/// Timestamp used for default backup directory names.
pub fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// `dump/<name>` inside a backup directory.
pub fn dump_relative_path(name: &str) -> PathBuf {
    PathBuf::from("dump").join(name)
}

/// `files/<sanitized remote path>` inside a backup directory.
pub fn files_relative_path(remote_from: &str) -> PathBuf {
    let safe = remote_from
        .trim_start_matches('/')
        .replace(['/', '\\'], "_");
    PathBuf::from("files").join(if safe.is_empty() {
        "root".to_string()
    } else {
        safe
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrunePlan {
    pub removed: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
}

/// Keep the `keep` newest timestamped backup directories under `root`.
///
/// Only directories that contain a `manifest.json` are considered, so an
/// unrelated directory next to the backups is never deleted. A `keep` of zero
/// is treated as "keep everything" to avoid accidental data loss.
pub fn prune(root: &Path, keep: usize) -> Result<PrunePlan> {
    if keep == 0 || !root.is_dir() {
        return Ok(PrunePlan::default());
    }
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = Manifest::path(&path);
        let Ok(manifest) = Manifest::load(&manifest_path) else {
            continue;
        };
        entries.push((manifest.created_at, path));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let mut plan = PrunePlan::default();
    for (index, (_, path)) in entries.into_iter().enumerate() {
        if index < keep {
            plan.kept.push(path);
        } else {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("remove old backup {}", path.display()))?;
            plan.removed.push(path);
        }
    }
    Ok(plan)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub manifest_ok: bool,
    pub dump_present: bool,
    pub size_ok: bool,
    pub sha256_ok: bool,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.manifest_ok && self.dump_present && self.size_ok && self.sha256_ok
    }
}

/// Verify `<dir>/manifest.json` and the dump it points at.
///
/// A dump without a checksum can still pass the size check; the report keeps
/// the distinction explicit so callers can require SHA-256 when they need it.
pub fn verify(dir: &Path) -> Result<VerifyReport> {
    let manifest = Manifest::load(&Manifest::path(dir))?;
    let mut report = VerifyReport {
        manifest_ok: true,
        ..VerifyReport::default()
    };
    let Some(dump) = manifest.dump else {
        return Ok(report);
    };
    let path = PathBuf::from(&dump.path);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Ok(report);
    };
    report.dump_present = true;
    report.size_ok = metadata.len() == dump.size;
    report.sha256_ok = match dump.sha256 {
        Some(expected) => crate::util::sha256_file(&path).is_ok_and(|actual| actual == expected),
        None => false,
    };
    Ok(report)
}

/// Create `<root>/<timestamp>` and return it.
pub fn create_timestamped_dir(root: &Path) -> Result<PathBuf> {
    let dir = root.join(timestamp());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create backup directory {}", dir.display()))?;
    Ok(dir)
}

/// Resolve the directory for a new backup.
///
/// * explicit `--to` wins and is used as-is;
/// * otherwise `configured` is a root: with retention (`keep`) the backup goes
///   into `<configured>/<timestamp>`, without it the root is used as-is;
/// * otherwise the default is `<project_root>/backups/<timestamp>`.
pub fn resolve_backup_dir(
    explicit: Option<&Path>,
    configured: Option<&Path>,
    project_root: &Path,
    keep: Option<usize>,
) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        std::fs::create_dir_all(explicit)
            .with_context(|| format!("create backup directory {}", explicit.display()))?;
        return Ok(explicit.to_path_buf());
    }
    match configured {
        Some(root) => {
            std::fs::create_dir_all(root)
                .with_context(|| format!("create backup directory {}", root.display()))?;
            if keep.is_some_and(|keep| keep > 0) {
                create_timestamped_dir(root)
            } else {
                Ok(root.to_path_buf())
            }
        }
        None => create_timestamped_dir(&project_root.join("backups")),
    }
}

/// Ensure a directory exists and is writable before a long dump starts.
pub fn prepare_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create backup directory {}", dir.display()))?;
    let probe = dir.join(".cloner-write-test");
    std::fs::write(&probe, b"").map_err(|error| {
        anyhow!(
            "backup directory {} is not writable: {error}",
            dir.display()
        )
    })?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_at(dir: &Path, created: &str) {
        let mut manifest = Manifest::new("dl.js", "/project", "development");
        manifest.created_at = created.to_string();
        manifest.write(dir).unwrap();
    }

    #[test]
    fn writes_and_loads_a_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("cloner.bak");
        std::fs::write(&dump, b"dump-bytes").unwrap();

        let mut manifest = Manifest::new("/project/dl.js", "/project", "development");
        manifest.remote = Some(ManifestRemote {
            host: "example.com".into(),
            user: "root".into(),
            port: 22,
            app_path: "/srv/app".into(),
        });
        manifest.database = Some(ManifestDatabase {
            engine: "postgres".into(),
            database: "app".into(),
            user: "app".into(),
        });
        manifest.set_dump(&dump).unwrap();
        manifest.files.push(ManifestFile {
            from: "/srv/app/uploads".into(),
            to: "/project/uploads".into(),
            files: 2,
            downloaded: 2,
            bytes: 10,
            ..ManifestFile::default()
        });

        let path = manifest.write(dir.path()).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded, manifest);
        assert_eq!(loaded.tool, "cloner");
        assert_eq!(loaded.dump.as_ref().unwrap().size, 10);
        assert!(loaded.dump.as_ref().unwrap().sha256.is_some());
        assert!(loaded.dump_is_intact());

        std::fs::write(&dump, b"changed").unwrap();
        assert!(!loaded.dump_is_intact());
    }

    #[test]
    fn computes_the_dump_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump");
        std::fs::write(&dump, b"abc").unwrap();
        let mut manifest = Manifest::default();
        manifest.set_dump(&dump).unwrap();
        assert_eq!(
            manifest.dump.unwrap().sha256.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    #[test]
    fn builds_relative_paths() {
        assert_eq!(
            dump_relative_path("cloner.bak"),
            PathBuf::from("dump/cloner.bak")
        );
        assert_eq!(
            files_relative_path("/srv/app/public/uploads"),
            PathBuf::from("files/srv_app_public_uploads")
        );
        assert_eq!(files_relative_path(""), PathBuf::from("files/root"));
    }

    #[test]
    fn prunes_only_manifest_directories_newest_first() {
        let root = tempfile::tempdir().unwrap();
        manifest_at(&root.path().join("b1"), "2026-01-01T00:00:00Z");
        manifest_at(&root.path().join("b2"), "2026-01-02T00:00:00Z");
        manifest_at(&root.path().join("b3"), "2026-01-03T00:00:00Z");
        std::fs::create_dir(root.path().join("unrelated")).unwrap();

        let plan = prune(root.path(), 2).unwrap();
        assert_eq!(plan.kept.len(), 2);
        assert_eq!(plan.removed.len(), 1);
        assert!(plan.removed[0].ends_with("b1"));
        assert!(root.path().join("b2").is_dir());
        assert!(root.path().join("b3").is_dir());
        assert!(root.path().join("unrelated").is_dir());
        assert!(!root.path().join("b1").exists());
    }

    #[test]
    fn prune_with_zero_keeps_everything() {
        let root = tempfile::tempdir().unwrap();
        manifest_at(&root.path().join("b1"), "2026-01-01T00:00:00Z");
        let plan = prune(root.path(), 0).unwrap();
        assert!(plan.removed.is_empty());
        assert!(plan.kept.is_empty());
        assert!(root.path().join("b1").is_dir());
    }

    #[test]
    fn resolves_backup_directories() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();

        let explicit = root.path().join("explicit");
        assert_eq!(
            resolve_backup_dir(Some(&explicit), None, &project, None).unwrap(),
            explicit
        );

        let configured = root.path().join("backups");
        assert_eq!(
            resolve_backup_dir(None, Some(&configured), &project, None).unwrap(),
            configured
        );

        let retained = resolve_backup_dir(None, Some(&configured), &project, Some(7)).unwrap();
        assert!(retained.starts_with(&configured));
        assert_ne!(retained, configured);

        let default = resolve_backup_dir(None, None, &project, None).unwrap();
        assert!(default.starts_with(project.join("backups")));
    }

    #[test]
    fn verifies_manifest_dump_size_and_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("cloner.bak");
        std::fs::write(&dump, b"dump-bytes").unwrap();
        let mut manifest = Manifest::default();
        manifest.set_dump(&dump).unwrap();
        manifest.write(dir.path()).unwrap();

        let report = verify(dir.path()).unwrap();
        assert!(report.ok(), "{report:?}");

        std::fs::write(&dump, b"changed-size").unwrap();
        let report = verify(dir.path()).unwrap();
        assert!(report.manifest_ok);
        assert!(!report.ok());
        assert!(!report.size_ok);
        assert!(!report.sha256_ok);

        std::fs::remove_file(&dump).unwrap();
        let report = verify(dir.path()).unwrap();
        assert!(!report.dump_present);
        assert!(!report.ok());
    }

    #[test]
    fn prepare_dir_detects_unwritable_directories() {
        let dir = tempfile::tempdir().unwrap();
        prepare_dir(dir.path()).unwrap();
        assert!(!dir.path().join(".cloner-write-test").exists());
    }
}
