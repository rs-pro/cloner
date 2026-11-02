//! Local command execution and POSIX shell quoting.
//!
//! Commands are always built with [`shq`] (POSIX quoting), so every platform
//! runs them through a POSIX shell. On Windows that shell comes from Git for
//! Windows unless `CLONER_SHELL` points at one.

use anyhow::{Context, Result, anyhow, bail};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Quote one argument for a POSIX shell.
pub fn shq(value: &str) -> String {
    shell_words::quote(value).into_owned()
}

/// Keep the last `limit` bytes of a command's output for error messages,
/// without splitting a UTF-8 character.
pub fn tail(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

#[derive(Debug, Clone, Default)]
pub struct LocalCmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl LocalCmdOutput {
    /// Best available error detail: stderr, then stdout.
    pub fn detail(&self) -> &str {
        if !self.stderr.trim().is_empty() {
            self.stderr.trim()
        } else {
            self.stdout.trim()
        }
    }
}

/// The shell used to run local commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalShell {
    pub program: PathBuf,
}

pub fn local_shell() -> Result<LocalShell> {
    let program = std::env::var_os("CLONER_SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(default_shell)
        .ok_or_else(|| {
            anyhow!(
                "no POSIX shell found for local commands; install Git for Windows \
                 (https://git-scm.com/downloads), which provides sh.exe, or set CLONER_SHELL"
            )
        })?;
    Ok(LocalShell { program })
}

#[cfg(not(windows))]
fn default_shell() -> Option<PathBuf> {
    find_in_path("sh").or_else(|| first_existing(&[PathBuf::from("/bin/sh")]))
}

#[cfg(windows)]
fn default_shell() -> Option<PathBuf> {
    for name in ["sh", "bash"] {
        if let Some(found) = find_in_path(name) {
            return Some(found);
        }
    }
    // Git for Windows ships sh.exe next to the git the user already has.
    let mut candidates = Vec::new();
    for env_name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(env_name) {
            let base = PathBuf::from(base);
            candidates.push(base.join(r"Git\bin\sh.exe"));
            candidates.push(base.join(r"Git\usr\bin\sh.exe"));
        }
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(base).join(r"Programs\Git\bin\sh.exe"));
    }
    first_existing(&candidates)
}

/// First path that exists as a file.
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let entries = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&entries) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        if dir
            .file_name()
            .is_some_and(|part| part.eq_ignore_ascii_case("WindowsApps"))
        {
            // App Execution Aliases (e.g. the WSL `bash` stub) are reparse
            // points, not usable shells.
            continue;
        }
        #[cfg(windows)]
        let candidates = vec![dir.join(name), dir.join(format!("{name}.exe"))];
        #[cfg(not(windows))]
        let candidates = vec![dir.join(name)];
        if let Some(found) = first_existing(&candidates) {
            return Some(found);
        }
    }
    None
}

/// Run a command and capture its output without failing on a non-zero exit.
pub fn run_capture(
    command: &str,
    cwd: Option<&std::path::Path>,
    envs: &[(String, String)],
    stdin_path: Option<&std::path::Path>,
) -> Result<LocalCmdOutput> {
    let shell = local_shell()?;
    let mut process = Command::new(&shell.program);
    process.arg("-c").arg(command);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    for (key, value) in envs {
        process.env(key, value);
    }
    if let Some(stdin_path) = stdin_path {
        let file = std::fs::File::open(stdin_path)
            .with_context(|| format!("open stdin {}", stdin_path.display()))?;
        process.stdin(Stdio::from(file));
    }
    let output = process
        .output()
        .with_context(|| format!("run local command: {command}"))?;
    Ok(LocalCmdOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

/// Run a command, printing it instead when `dry_run` is set.
pub fn run(
    command: &str,
    cwd: Option<&std::path::Path>,
    envs: &[(String, String)],
    stdin_path: Option<&std::path::Path>,
    dry_run: bool,
) -> Result<LocalCmdOutput> {
    log::debug!("local: {command}");
    if dry_run {
        println!("DRY RUN local: {command}");
        return Ok(LocalCmdOutput::default());
    }
    let result = run_capture(command, cwd, envs, stdin_path)?;
    if result.code != 0 {
        bail!(
            "local command failed with exit code {}: {command}\n{}",
            result.code,
            tail(result.detail(), 4000)
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn quotes_shell_arguments() {
        assert_eq!(shq("plain"), "plain");
        assert_eq!(shq("two words"), "'two words'");
        assert_eq!(shq(""), "''");
        assert_eq!(shq("it's"), "'it'\\''s'");
        assert_eq!(shq("/tmp/a b/c"), "'/tmp/a b/c'");
    }

    #[test]
    fn picks_the_first_existing_shell_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("sh.exe");
        std::fs::write(&shell, b"").unwrap();
        let missing = dir.path().join("bash.exe");
        assert_eq!(
            first_existing(&[missing.clone(), shell.clone()]),
            Some(shell.clone())
        );
        assert_eq!(first_existing(&[missing]), None);
    }

    #[test]
    fn resolves_a_local_shell() {
        let shell = local_shell().expect("a POSIX shell is available");
        assert!(
            shell.program == Path::new("sh") || shell.program.is_file(),
            "unexpected shell {}",
            shell.program.display()
        );
    }

    #[test]
    fn captures_stdout_stderr_and_exit_codes() {
        let ok = run_capture("echo hello", None, &[], None).unwrap();
        assert_eq!(ok.stdout.trim(), "hello");
        assert_eq!(ok.code, 0);

        let failed = run_capture("echo oops >&2; exit 7", None, &[], None).unwrap();
        assert_eq!(failed.code, 7);
        assert_eq!(failed.stderr.trim(), "oops");
    }

    #[test]
    fn captures_with_cwd_env_and_stdin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "content").unwrap();
        let output = run_capture(
            "cat marker; printf '%s' \"$CLONER_TEST\"",
            Some(dir.path()),
            &[("CLONER_TEST".to_string(), "value".to_string())],
            None,
        )
        .unwrap();
        assert_eq!(output.stdout, "contentvalue");

        let input = dir.path().join("input.txt");
        std::fs::write(&input, "stdin-data").unwrap();
        let output = run_capture("cat", None, &[], Some(&input)).unwrap();
        assert_eq!(output.stdout, "stdin-data");
    }

    #[test]
    fn dry_run_returns_success_without_executing() {
        let output = run("exit 1", None, &[], None, true).unwrap();
        assert_eq!(output.code, 0);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn errors_include_command_and_output() {
        let error = run("echo boom >&2; exit 3", None, &[], None, false).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("exit code 3"));
        assert!(text.contains("boom"));
        assert!(text.contains("echo boom"));
    }

    #[test]
    fn tail_keeps_utf8_boundaries() {
        assert_eq!(tail("short", 100), "short");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("русский", 8), "ский");
    }
}
