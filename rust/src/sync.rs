use anyhow::{Context, Result, anyhow};
use globset::{Glob, GlobSet, GlobSetBuilder};
use russh_sftp::client::SftpSession;
use russh_sftp::client::fs::Metadata;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;

use crate::config::ResolvedFileSpec;
use crate::util::human_size;

#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    pub files: u64,
    pub dirs: u64,
    pub downloaded: u64,
    pub skipped: u64,
    pub deleted: u64,
    pub bytes: u64,
    pub errors: u64,
}

#[derive(Debug)]
pub struct Filters {
    include: Option<GlobSet>,
    exclude: GlobSet,
}

impl Filters {
    pub fn new(spec: &ResolvedFileSpec) -> Result<Self> {
        let include = if spec.include.is_empty() {
            None
        } else {
            Some(build_globs(&spec.include)?)
        };
        let exclude = build_globs(&spec.exclude)?;
        Ok(Self { include, exclude })
    }

    fn allows(&self, rel: &str) -> bool {
        let base = rel.rsplit('/').next().unwrap_or(rel);
        if self.exclude.is_match(rel) || self.exclude.is_match(base) {
            return false;
        }
        if let Some(include) = &self.include {
            return include.is_match(rel) || include.is_match(base);
        }
        true
    }
}

fn build_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid glob pattern {pattern:?}"))?);
    }
    builder.build().context("build glob set")
}

pub async fn sync(sftp: &SftpSession, spec: &ResolvedFileSpec, dry_run: bool) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let filters = Filters::new(spec)?;
    let root_meta = sftp
        .metadata(&spec.from)
        .await
        .with_context(|| format!("stat remote {}", spec.from))?;

    log::info!(
        "sync {} -> {}{}",
        spec.from,
        spec.to.display(),
        if dry_run { " (dry run)" } else { "" }
    );

    if root_meta.is_dir() {
        if !dry_run {
            tokio::fs::create_dir_all(&spec.to)
                .await
                .with_context(|| format!("create {}", spec.to.display()))?;
        }
        let mut expected = HashSet::new();
        sync_dir(
            sftp,
            &spec.from,
            &spec.to,
            "",
            spec,
            &filters,
            dry_run,
            &mut stats,
            &mut expected,
        )
        .await?;
        if spec.delete && !dry_run {
            delete_extra(&spec.to, &expected, &mut stats).await?;
        }
    } else {
        let name = spec.from.rsplit('/').next().unwrap_or("download");
        let dest = if spec.to.is_dir()
            || spec
                .to
                .to_string_lossy()
                .ends_with(std::path::MAIN_SEPARATOR)
        {
            spec.to.join(name)
        } else {
            spec.to.clone()
        };
        if !dry_run && let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        download_file(sftp, &spec.from, &dest, &root_meta, dry_run, &mut stats).await?;
    }

    log::info!(
        "sync done: {} files, {} dirs, {} downloaded ({}), {} skipped, {} deleted, {} errors",
        stats.files,
        stats.dirs,
        stats.downloaded,
        human_size(stats.bytes),
        stats.skipped,
        stats.deleted,
        stats.errors
    );
    if stats.errors > 0 {
        anyhow::bail!("{} file(s) failed to sync", stats.errors);
    }
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
async fn sync_dir(
    sftp: &SftpSession,
    remote_dir: &str,
    local_dir: &Path,
    rel_dir: &str,
    spec: &ResolvedFileSpec,
    filters: &Filters,
    dry_run: bool,
    stats: &mut SyncStats,
    expected: &mut HashSet<PathBuf>,
) -> Result<()> {
    let entries = sftp
        .read_dir(remote_dir)
        .await
        .with_context(|| format!("list remote directory {remote_dir}"))?;
    for entry in entries {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let rel = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        if !filters.allows(&rel) {
            log::debug!("skip filtered {rel}");
            continue;
        }
        let remote_path = entry.path();
        let local_path = local_dir.join(&name);
        let meta = entry.metadata();

        if meta.is_dir() {
            stats.dirs += 1;
            expected.insert(local_path.clone());
            if !dry_run {
                tokio::fs::create_dir_all(&local_path)
                    .await
                    .with_context(|| format!("create {}", local_path.display()))?;
                apply_permissions(&local_path, &meta);
            }
            Box::pin(sync_dir(
                sftp,
                &remote_path,
                &local_path,
                &rel,
                spec,
                filters,
                dry_run,
                stats,
                expected,
            ))
            .await?;
        } else if meta.is_symlink() && !spec.follow_symlinks {
            download_symlink(sftp, &remote_path, &local_path, dry_run, stats).await?;
            expected.insert(local_path);
        } else {
            stats.files += 1;
            expected.insert(local_path.clone());
            if let Some(max) = spec.max_size
                && meta.len() > max
            {
                log::warn!(
                    "skip {rel}: {} exceeds maxSize {}",
                    human_size(meta.len()),
                    human_size(max)
                );
                stats.skipped += 1;
                continue;
            }
            download_file(sftp, &remote_path, &local_path, &meta, dry_run, stats).await?;
        }
    }
    Ok(())
}

async fn download_file(
    sftp: &SftpSession,
    remote: &str,
    local: &Path,
    meta: &Metadata,
    dry_run: bool,
    stats: &mut SyncStats,
) -> Result<()> {
    if local_is_current(local, meta) {
        stats.skipped += 1;
        log::debug!("unchanged {}", local.display());
        return Ok(());
    }
    if dry_run {
        stats.downloaded += 1;
        stats.bytes += meta.len();
        log::info!(
            "would download {} ({})",
            local.display(),
            human_size(meta.len())
        );
        return Ok(());
    }
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = local.with_file_name(format!(
        ".{}.cloner-part",
        local.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let mut remote_file = sftp
        .open(remote)
        .await
        .with_context(|| format!("open remote file {remote}"))?;
    let mut local_file = tokio::fs::File::create(&tmp)
        .await
        .with_context(|| format!("create {}", tmp.display()))?;
    let copied = tokio::io::copy(&mut remote_file, &mut local_file)
        .await
        .with_context(|| format!("download {remote}"))?;
    local_file.flush().await?;
    local_file.sync_all().await?;
    drop(local_file);
    tokio::fs::rename(&tmp, local)
        .await
        .with_context(|| format!("rename {} to {}", tmp.display(), local.display()))?;
    apply_permissions(local, meta);
    apply_times(local, meta);
    stats.downloaded += 1;
    stats.bytes += copied;
    log::debug!("downloaded {} ({})", local.display(), human_size(copied));
    Ok(())
}

async fn download_symlink(
    sftp: &SftpSession,
    remote: &str,
    local: &Path,
    dry_run: bool,
    stats: &mut SyncStats,
) -> Result<()> {
    let target = sftp
        .read_link(remote)
        .await
        .with_context(|| format!("read symlink {remote}"))?;
    if dry_run {
        log::info!("would symlink {} -> {}", local.display(), target);
        stats.downloaded += 1;
        return Ok(());
    }
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if let Ok(existing) = tokio::fs::read_link(local).await {
        if existing.to_string_lossy() == target {
            stats.skipped += 1;
            return Ok(());
        }
        tokio::fs::remove_file(local).await.ok();
    } else if local.is_file() {
        #[cfg(unix)]
        tokio::fs::remove_file(local).await.ok();
        #[cfg(not(unix))]
        {
            // Replacing a regular file with a symlink needs privileges on
            // Windows; keep the file and report the skip.
            log::warn!(
                "keeping local file {} (cannot replace it with a symlink)",
                local.display()
            );
            stats.skipped += 1;
            return Ok(());
        }
    }
    #[cfg(unix)]
    let created = tokio::fs::symlink(&target, local).await;
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_file(&target, local)
        .or_else(|_| std::os::windows::fs::symlink_dir(&target, local))
        .map_err(|error| anyhow!(error));
    match created {
        Ok(()) => stats.downloaded += 1,
        Err(error) => {
            // Windows only allows unprivileged symlinks in Developer Mode.
            log::warn!(
                "cannot create symlink {} -> {}: {error}",
                local.display(),
                target
            );
            stats.skipped += 1;
        }
    }
    Ok(())
}

fn local_is_current(local: &Path, meta: &Metadata) -> bool {
    let Ok(local_meta) = std::fs::metadata(local) else {
        return false;
    };
    if !local_meta.is_file() || local_meta.len() != meta.len() {
        return false;
    }
    let Ok(remote_mtime) = meta.modified() else {
        return true;
    };
    let Ok(local_mtime) = local_meta.modified() else {
        return true;
    };
    match remote_mtime.duration_since(local_mtime) {
        Ok(diff) => diff < Duration::from_secs(2),
        Err(_) => local_mtime
            .duration_since(remote_mtime)
            .map(|d| d < Duration::from_secs(2))
            .unwrap_or(false),
    }
}

fn apply_permissions(path: &Path, meta: &Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = meta.permissions();
        let mut mode = 0o600;
        if perms.owner_read {
            mode |= 0o400;
        }
        if perms.owner_write {
            mode |= 0o200;
        }
        if perms.owner_exec {
            mode |= 0o100;
        }
        if perms.group_read {
            mode |= 0o040;
        }
        if perms.group_write {
            mode |= 0o020;
        }
        if perms.group_exec {
            mode |= 0o010;
        }
        if perms.other_read {
            mode |= 0o004;
        }
        if perms.other_write {
            mode |= 0o002;
        }
        if perms.other_exec {
            mode |= 0o001;
        }
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        // Windows has no POSIX mode bits; new files inherit the ACLs of the
        // parent directory.
        let _ = (path, meta);
    }
}

fn apply_times(path: &Path, meta: &Metadata) {
    if let Ok(modified) = meta.modified()
        && let Ok(file) = std::fs::OpenOptions::new().write(true).open(path)
    {
        let times = std::fs::FileTimes::new().set_modified(modified);
        let _ = file.set_times(times);
    }
}

async fn delete_extra(
    local_root: &Path,
    expected: &HashSet<PathBuf>,
    stats: &mut SyncStats,
) -> Result<()> {
    let mut stack = vec![local_root.to_path_buf()];
    let mut remove = Vec::new();
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!("cannot read {}: {error}", dir.display());
                continue;
            }
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(path.clone());
                if !expected.contains(&path) {
                    remove.push(path);
                }
            } else if !expected.contains(&path) {
                remove.push(path);
            }
        }
    }
    remove.sort();
    remove.reverse();
    for path in remove {
        if path == local_root {
            continue;
        }
        if path.is_dir() {
            if let Err(error) = tokio::fs::remove_dir_all(&path).await {
                log::warn!("cannot remove {}: {error}", path.display());
            } else {
                stats.deleted += 1;
            }
        } else if let Err(error) = tokio::fs::remove_file(&path).await {
            log::warn!("cannot remove {}: {error}", path.display());
        } else {
            stats.deleted += 1;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn now() -> SystemTime {
    SystemTime::now()
}

#[allow(dead_code)]
fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| anyhow!("create {}: {e}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(include: Vec<&str>, exclude: Vec<&str>) -> ResolvedFileSpec {
        ResolvedFileSpec {
            from: "/srv/app/uploads".into(),
            to: PathBuf::from("/tmp/uploads"),
            delete: false,
            exclude: exclude.into_iter().map(str::to_string).collect(),
            include: include.into_iter().map(str::to_string).collect(),
            max_size: None,
            follow_symlinks: false,
        }
    }

    #[test]
    fn default_filters_allow_everything() {
        let filters = Filters::new(&spec(vec![], vec![])).unwrap();
        assert!(filters.allows("a.txt"));
        assert!(filters.allows("nested/b.txt"));
    }

    #[test]
    fn exclude_matches_paths_and_basenames() {
        let filters = Filters::new(&spec(vec![], vec!["*.tmp", "cache/**"])).unwrap();
        assert!(!filters.allows("a.tmp"));
        assert!(!filters.allows("nested/a.tmp"));
        assert!(!filters.allows("cache/x/y"));
        assert!(filters.allows("nested/a.txt"));
    }

    #[test]
    fn include_restricts_to_matching_files() {
        let filters = Filters::new(&spec(vec!["*.jpg", "docs/**"], vec![])).unwrap();
        assert!(filters.allows("photo.jpg"));
        assert!(filters.allows("nested/photo.jpg"));
        assert!(filters.allows("docs/readme.md"));
        assert!(!filters.allows("photo.png"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let filters = Filters::new(&spec(vec!["*.jpg"], vec!["draft*"])).unwrap();
        assert!(filters.allows("keep.jpg"));
        assert!(!filters.allows("draft.jpg"));
    }

    #[test]
    fn rejects_invalid_glob_patterns() {
        let error = Filters::new(&spec(vec![], vec!["[unclosed"])).unwrap_err();
        assert!(format!("{error:#}").contains("invalid glob pattern"));
    }

    #[test]
    fn stats_default_to_zero() {
        let stats = SyncStats::default();
        assert_eq!(stats.files, 0);
        assert_eq!(stats.dirs, 0);
        assert_eq!(stats.downloaded, 0);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.deleted, 0);
        assert_eq!(stats.bytes, 0);
        assert_eq!(stats.errors, 0);
    }

    #[test]
    fn now_is_after_unix_epoch() {
        let elapsed = now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch");
        assert!(elapsed.as_secs() > 0);
    }
}
