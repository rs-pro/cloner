use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Quote one argument for a POSIX shell.
pub fn shq(value: &str) -> String {
    shell_words::quote(value).into_owned()
}

/// Join a remote POSIX path without touching the local filesystem.
pub fn remote_join(base: &str, child: &str) -> String {
    if base.is_empty() || base == "." {
        return child.to_string();
    }
    if child.is_empty() {
        return base.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        child.trim_start_matches('/')
    )
}

pub fn expand_tilde(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(value)
}

pub fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = expand_tilde(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// Parse a dotenv-style file. Supports comments, `export`, single/double
/// quotes and the common `\n`, `\r`, `\t` escapes inside double quotes.
pub fn parse_env(content: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut lines = content.lines().peekable();
    while let Some(raw) = lines.next() {
        let mut line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("export ") {
            line = rest.trim_start();
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut value = value.trim().to_string();

        // Multi-line double quoted values are common in generated .env files.
        if value.starts_with('"') && !value.ends_with('"') {
            for next in lines.by_ref() {
                value.push('\n');
                value.push_str(next);
                if next.trim_end().ends_with('"') {
                    break;
                }
            }
        }

        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            let quote = value.chars().next().unwrap();
            let inner = &value[1..value.len() - 1];
            if quote == '"' {
                inner
                    .replace("\\n", "\n")
                    .replace("\\r", "\r")
                    .replace("\\t", "\t")
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\")
            } else {
                inner.to_string()
            }
        } else {
            // Strip an unquoted trailing comment introduced by whitespace.
            match value.find(" #") {
                Some(pos) => value[..pos].trim_end().to_string(),
                None => value,
            }
        };
        out.insert(key.to_string(), value);
    }
    out
}

pub fn read_env_file(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .map(|content| parse_env(&content))
        .unwrap_or_default()
}

/// Expand `${VAR}`, `${VAR:-default}`, `${VAR-default}`, `${VAR:?message}`,
/// `${VAR?message}` and bare `$VAR` against an environment map. Unknown
/// variables without a default expand to an empty string, like compose.
pub fn interpolate(input: &str, env: &BTreeMap<String, String>) -> Result<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if i + 1 >= chars.len() {
            out.push('$');
            break;
        }
        if chars[i + 1] == '$' {
            out.push('$');
            i += 2;
            continue;
        }
        if chars[i + 1] == '{' {
            let start = i + 2;
            let mut end = start;
            let mut depth = 1;
            while end < chars.len() {
                match chars[end] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                end += 1;
            }
            if end >= chars.len() {
                bail!("unterminated ${{...}} in {:?}", input);
            }
            let expr: String = chars[start..end].iter().collect();
            out.push_str(&expand_expr(&expr, env)?);
            i = end + 1;
            continue;
        }
        // Bare $NAME
        let start = i + 1;
        let mut end = start;
        while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
            end += 1;
        }
        if end == start {
            out.push('$');
            i += 1;
        } else {
            let key: String = chars[start..end].iter().collect();
            out.push_str(env.get(&key).map(String::as_str).unwrap_or(""));
            i = end;
        }
    }
    Ok(out)
}

fn expand_expr(expr: &str, env: &BTreeMap<String, String>) -> Result<String> {
    // NAME, NAME:-default, NAME-default, NAME:?message, NAME?message
    let (key, kind, default) = if let Some((k, d)) = expr.split_once(":-") {
        (k, ":-", Some(d))
    } else if let Some((k, d)) = expr.split_once('-') {
        (k, "-", Some(d))
    } else if let Some((k, d)) = expr.split_once(":?") {
        (k, ":?", Some(d))
    } else if let Some((k, d)) = expr.split_once('?') {
        (k, "?", Some(d))
    } else {
        (expr, "", None)
    };
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("invalid variable expression ${{{expr}}}");
    }
    let value = env.get(key);
    match kind {
        ":-" => Ok(value
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| default.unwrap_or("").to_string())),
        "-" => Ok(value
            .cloned()
            .unwrap_or_else(|| default.unwrap_or("").to_string())),
        ":?" => match value.filter(|v| !v.is_empty()) {
            Some(v) => Ok(v.clone()),
            None => Err(anyhow!(
                "required variable {key} is missing: {}",
                default.unwrap_or("")
            )),
        },
        "?" => match value {
            Some(v) => Ok(v.clone()),
            None => Err(anyhow!(
                "required variable {key} is missing: {}",
                default.unwrap_or("")
            )),
        },
        _ => Ok(value.cloned().unwrap_or_default()),
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dotenv_quotes_and_comments() {
        let env = parse_env(
            "# comment\nexport A=plain\nB=\"two words\"\nC='single # not comment'\nD=value # comment\n",
        );
        assert_eq!(env.get("A").map(String::as_str), Some("plain"));
        assert_eq!(env.get("B").map(String::as_str), Some("two words"));
        assert_eq!(
            env.get("C").map(String::as_str),
            Some("single # not comment")
        );
        assert_eq!(env.get("D").map(String::as_str), Some("value"));
    }

    #[test]
    fn interpolates_compose_style_variables() {
        let mut env = BTreeMap::new();
        env.insert("SET".to_string(), "yes".to_string());
        env.insert("EMPTY".to_string(), String::new());
        assert_eq!(interpolate("${SET}", &env).unwrap(), "yes");
        assert_eq!(
            interpolate("${MISSING:-fallback}", &env).unwrap(),
            "fallback"
        );
        assert_eq!(interpolate("${EMPTY:-fallback}", &env).unwrap(), "fallback");
        assert_eq!(interpolate("${EMPTY-fallback}", &env).unwrap(), "");
        assert!(interpolate("${MISSING:?required}", &env).is_err());
    }

    #[test]
    fn quotes_shell_arguments() {
        assert_eq!(shq("plain"), "plain");
        assert_eq!(shq("two words"), "'two words'");
        assert_eq!(shq(""), "''");
        assert_eq!(shq("it's"), "'it'\\''s'");
        assert_eq!(shq("/tmp/a b/c"), "'/tmp/a b/c'");
    }

    #[test]
    fn joins_remote_paths() {
        assert_eq!(remote_join("/srv/app", "tmp"), "/srv/app/tmp");
        assert_eq!(remote_join("/srv/app/", "tmp"), "/srv/app/tmp");
        assert_eq!(remote_join("/srv/app", "/tmp"), "/srv/app/tmp");
        assert_eq!(remote_join("", "tmp"), "tmp");
        assert_eq!(remote_join(".", "tmp"), "tmp");
        assert_eq!(remote_join("/srv/app", ""), "/srv/app");
    }

    #[test]
    fn resolves_relative_and_absolute_paths() {
        let base = Path::new("/project");
        assert_eq!(
            resolve_path(base, "uploads"),
            PathBuf::from("/project/uploads")
        );
        assert_eq!(
            resolve_path(base, "/var/lib/app"),
            PathBuf::from("/var/lib/app")
        );
        assert_eq!(
            resolve_path(base, "./dev-docker/.env"),
            PathBuf::from("/project/./dev-docker/.env")
        );
    }

    #[test]
    fn expands_tilde_using_home() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let expanded = expand_tilde("~/.ssh/id_ed25519");
        match home {
            Some(home) => assert_eq!(expanded, home.join(".ssh/id_ed25519")),
            None => assert_eq!(expanded, PathBuf::from("~/.ssh/id_ed25519")),
        }
        assert_eq!(expand_tilde("~user/file"), PathBuf::from("~user/file"));
    }

    #[test]
    fn parses_multiline_and_escaped_dotenv_values() {
        let env = parse_env(
            "MULTI=\"first\nsecond\"\nESC=\"a\\tb\\nc\\\"d\\\\e\"\nEMPTY=\n  SPACED = value  \n",
        );
        assert_eq!(env.get("MULTI").map(String::as_str), Some("first\nsecond"));
        assert_eq!(env.get("ESC").map(String::as_str), Some("a\tb\nc\"d\\e"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(env.get("SPACED").map(String::as_str), Some("value"));
    }

    #[test]
    fn ignores_invalid_dotenv_lines() {
        let env = parse_env("NO_EQUALS\nBAD-KEY=x\n# comment\nGOOD=1\n");
        assert_eq!(env.len(), 1);
        assert_eq!(env.get("GOOD").map(String::as_str), Some("1"));
        assert!(!env.contains_key("BAD-KEY"));
    }

    #[test]
    fn reads_missing_env_file_as_empty() {
        assert!(read_env_file(Path::new("/definitely/not/here/.env")).is_empty());
    }

    #[test]
    fn interpolates_bare_dollars_and_escapes() {
        let mut env = BTreeMap::new();
        env.insert("NAME".to_string(), "value".to_string());
        assert_eq!(interpolate("$NAME", &env).unwrap(), "value");
        assert_eq!(interpolate("$$NAME", &env).unwrap(), "$NAME");
        assert_eq!(interpolate("a$b", &env).unwrap(), "a");
        assert_eq!(interpolate("100$", &env).unwrap(), "100$");
        assert_eq!(interpolate("${NAME}!", &env).unwrap(), "value!");
        assert_eq!(interpolate("no vars", &env).unwrap(), "no vars");
    }

    #[test]
    fn interpolate_rejects_malformed_expressions() {
        let env = BTreeMap::new();
        assert!(interpolate("${UNTERMINATED", &env).is_err());
        assert!(interpolate("${bad name}", &env).is_err());
    }

    #[test]
    fn computes_sha256_of_files() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            sha256_file(&empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let abc = dir.path().join("abc");
        std::fs::write(&abc, b"abc").unwrap();
        assert_eq!(
            sha256_file(&abc).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        assert!(sha256_file(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn formats_human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(human_size(2 * 1024_u64.pow(4)), "2.0 TiB");
    }
}
