//! Restore a stored backup instead of dumping live production again.
//!
//! This is the entry point the RSBackup control plane can call after it has
//! downloaded a backup artifact: reset the local database according to the DSL
//! and pipe the stored dump into it.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::backup::{self, Manifest};
use crate::config::{ResolvedDb, ResolvedDbConfig, ResolvedLocal};
use crate::db;

/// A backup directory (`manifest.json` + `dump/`) or a dump file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBackup {
    pub dump: PathBuf,
    pub manifest: Option<Manifest>,
    pub directory: Option<PathBuf>,
}

impl StoredBackup {
    pub fn resolve(source: &Path) -> Result<Self> {
        if source.is_file() {
            return Ok(Self {
                dump: source.to_path_buf(),
                manifest: None,
                directory: None,
            });
        }
        if !source.is_dir() {
            bail!("backup source {} does not exist", source.display());
        }
        let manifest_path = Manifest::path(source);
        let manifest = Manifest::load(&manifest_path).with_context(|| {
            format!(
                "{} is neither a dump file nor a backup directory with {}",
                source.display(),
                backup::MANIFEST_FILE
            )
        })?;
        let dump = match &manifest.dump {
            Some(dump) => {
                let recorded = PathBuf::from(&dump.path);
                if recorded.is_file() {
                    recorded
                } else {
                    source.join(backup::dump_relative_path(
                        recorded
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "dump".to_string())
                            .as_str(),
                    ))
                }
            }
            None => bail!(
                "{} has no dump entry; pass the dump file explicitly",
                manifest_path.display()
            ),
        };
        if !dump.is_file() {
            bail!(
                "dump {} referenced by {} is missing",
                dump.display(),
                manifest_path.display()
            );
        }
        Ok(Self {
            dump,
            manifest: Some(manifest),
            directory: Some(source.to_path_buf()),
        })
    }

    pub fn engine(&self) -> Option<crate::engine::Engine> {
        self.manifest
            .as_ref()
            .and_then(|manifest| manifest.database.as_ref())
            .and_then(|database| crate::engine::Engine::parse(&database.engine))
    }

    pub fn verify(&self) -> Result<backup::VerifyReport> {
        match &self.directory {
            Some(directory) => backup::verify(directory),
            None => {
                let mut report = backup::VerifyReport {
                    manifest_ok: true,
                    dump_present: self.dump.is_file(),
                    ..backup::VerifyReport::default()
                };
                if report.dump_present {
                    let size = std::fs::metadata(&self.dump)?.len();
                    report.size_ok = size > 0;
                    report.sha256_ok = true;
                }
                Ok(report)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RestoreOptions {
    /// Reset the local database first (according to `database.reset`).
    pub reset: bool,
    pub dry_run: bool,
}

/// Restore a stored dump into the local database.
pub fn restore_local_backup(
    local: &ResolvedLocal,
    cfg: &ResolvedDbConfig,
    db: &ResolvedDb,
    source: &Path,
    project_root: &Path,
    options: RestoreOptions,
) -> Result<StoredBackup> {
    let stored = StoredBackup::resolve(source)?;
    let report = stored.verify()?;
    if !report.ok() {
        bail!(
            "backup {} failed verification: {report:?}",
            stored.dump.display()
        );
    }
    if let Some(engine) = stored.engine()
        && engine != db.engine
    {
        bail!(
            "backup was taken from {} but the local database is {}",
            engine.as_str(),
            db.engine.as_str()
        );
    }
    if options.reset {
        db::reset_local(local, cfg, db, project_root, options.dry_run)?;
    }
    db::restore_local(local, cfg, db, &stored.dump, options.dry_run)?;
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{ManifestDatabase, ManifestDump};

    fn write_backup(dir: &Path, with_dump: bool) -> Manifest {
        std::fs::create_dir_all(dir.join("dump")).unwrap();
        let dump_path = dir.join("dump/cloner.bak");
        if with_dump {
            std::fs::write(&dump_path, b"dump-bytes").unwrap();
        }
        let manifest = Manifest {
            database: Some(ManifestDatabase {
                engine: "postgres".into(),
                database: "app".into(),
                user: "app".into(),
            }),
            dump: Some(ManifestDump {
                path: dump_path.display().to_string(),
                size: 10,
                sha256: if with_dump {
                    crate::util::sha256_file(&dump_path).ok()
                } else {
                    None
                },
            }),
            ..Manifest::default()
        };
        manifest.write(dir).unwrap();
        manifest
    }

    #[test]
    fn resolves_a_dump_file() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("manual.sql");
        std::fs::write(&dump, b"data").unwrap();
        let stored = StoredBackup::resolve(&dump).unwrap();
        assert_eq!(stored.dump, dump);
        assert!(stored.manifest.is_none());
        assert!(stored.engine().is_none());
        assert!(stored.verify().unwrap().ok());
    }

    #[test]
    fn resolves_a_backup_directory_and_verifies_it() {
        let dir = tempfile::tempdir().unwrap();
        write_backup(dir.path(), true);
        let stored = StoredBackup::resolve(dir.path()).unwrap();
        assert_eq!(stored.dump, dir.path().join("dump/cloner.bak"));
        assert_eq!(stored.engine(), Some(crate::engine::Engine::Postgres));
        assert!(stored.verify().unwrap().ok());
        assert_eq!(stored.directory.as_deref(), Some(dir.path()));
    }

    #[test]
    fn rejects_missing_manifests_and_dumps() {
        let dir = tempfile::tempdir().unwrap();
        let error = StoredBackup::resolve(dir.path()).unwrap_err();
        assert!(error.to_string().contains("backup directory"));

        write_backup(dir.path(), false);
        let error = StoredBackup::resolve(dir.path()).unwrap_err();
        assert!(error.to_string().contains("missing"));
        assert!(StoredBackup::resolve(&dir.path().join("nope")).is_err());
    }
}
