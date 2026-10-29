use anyhow::{Context, Result, anyhow, bail};
use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh_sftp::client::SftpSession;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::config::ResolvedRemote;
use crate::util::{expand_tilde, shq};

pub struct Client {
    strict_host_key_checking: bool,
    known_hosts: Option<PathBuf>,
    host: String,
    port: u16,
}

impl client::Handler for Client {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            PublicKeyOrCertificate::Certificate(cert) => {
                log::warn!(
                    "accepting SSH certificate for {}:{} without known_hosts verification",
                    self.host,
                    self.port
                );
                let _ = cert;
                return Ok(true);
            }
        };
        let fingerprint = key.fingerprint(Default::default());
        if self.strict_host_key_checking {
            let path = self
                .known_hosts
                .clone()
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ssh/known_hosts"))
                })
                .ok_or_else(|| anyhow!("cannot locate known_hosts"))?;
            let known = russh::keys::check_known_hosts_path(&self.host, self.port, key, &path)
                .with_context(|| format!("checking host key against {}", path.display()))?;
            if !known {
                bail!(
                    "unknown SSH host key for {}:{} ({fingerprint}); add it to {} or set strictHostKeyChecking: false",
                    self.host,
                    self.port,
                    path.display()
                );
            }
            log::debug!(
                "host key for {}:{} verified: {fingerprint}",
                self.host,
                self.port
            );
        } else {
            log::warn!(
                "accepting SSH host key for {}:{} without known_hosts verification: {fingerprint}",
                self.host,
                self.port
            );
        }
        Ok(true)
    }
}

pub struct Ssh {
    handle: Handle<Client>,
}

#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: u32,
}

impl Ssh {
    pub async fn connect(cfg: &ResolvedRemote) -> Result<Self> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            ..Default::default()
        });
        let handler = Client {
            strict_host_key_checking: cfg.strict_host_key_checking,
            known_hosts: cfg.known_hosts.clone(),
            host: cfg.host.clone(),
            port: cfg.port,
        };
        log::info!("connecting to {}@{}:{}", cfg.user, cfg.host, cfg.port);
        let mut handle = client::connect(config, (cfg.host.as_str(), cfg.port), handler)
            .await
            .with_context(|| format!("connect to {}:{}", cfg.host, cfg.port))?;

        // Prefer the SSH agent when one is available (this is how the
        // developer's `ssh` command authenticates in practice).
        if std::env::var_os("SSH_AUTH_SOCK").is_some()
            && let Ok(mut agent) = russh::keys::agent::client::AgentClient::connect_env().await
        {
            match agent.request_identities().await {
                Ok(identities) => {
                    log::debug!("SSH agent offered {} identities", identities.len());
                    for identity in identities {
                        let public_key = identity.public_key().into_owned();
                        let hash = handle.best_supported_rsa_hash().await?.flatten();
                        match handle
                            .authenticate_publickey_with(
                                cfg.user.clone(),
                                public_key,
                                hash,
                                &mut agent,
                            )
                            .await
                        {
                            Ok(auth) if auth.success() => {
                                log::debug!("authenticated through SSH agent");
                                return Ok(Self { handle });
                            }
                            Ok(_) => continue,
                            Err(error) => {
                                log::debug!("SSH agent key rejected: {error}");
                                continue;
                            }
                        }
                    }
                }
                Err(error) => log::debug!("cannot list SSH agent identities: {error}"),
            }
        }

        let mut key_error = None;
        if let Some(key_path) = find_identity(cfg) {
            match load_secret_key(&key_path, key_passphrase().as_deref()) {
                Ok(key) => {
                    let hash = handle.best_supported_rsa_hash().await?.flatten();
                    let auth = handle
                        .authenticate_publickey(
                            cfg.user.clone(),
                            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                        )
                        .await
                        .with_context(|| {
                            format!("public key authentication using {}", key_path.display())
                        })?;
                    if auth.success() {
                        log::debug!("authenticated with {}", key_path.display());
                        return Ok(Self { handle });
                    }
                    key_error = Some(format!("public key {} was rejected", key_path.display()));
                }
                Err(error) => {
                    key_error = Some(format!("cannot load {}: {error}", key_path.display()));
                }
            }
        }

        if let Some(password) = cfg.password.as_ref().filter(|p| !p.is_empty()) {
            let auth = handle
                .authenticate_password(cfg.user.clone(), password.clone())
                .await
                .context("password authentication")?;
            if auth.success() {
                return Ok(Self { handle });
            }
            bail!("password authentication was rejected for {}", cfg.user);
        }

        Err(anyhow!(
            "no usable SSH authentication method for {}@{}:{} ({})",
            cfg.user,
            cfg.host,
            cfg.port,
            key_error.unwrap_or_else(|| "no identity file found".to_string())
        ))
    }

    pub async fn exec(&self, command: &str) -> Result<CmdOutput> {
        log::debug!("remote: {command}");
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .context("open SSH session channel")?;
        // Some hosts use fish (or another non-POSIX shell) as the login shell.
        // Always run through POSIX sh so `&&`, redirects and subshells behave
        // the same everywhere.
        let wrapped = format!("sh -lc {}", shq(command));
        channel
            .exec(true, wrapped)
            .await
            .with_context(|| format!("start remote command: {command}"))?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut code = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                russh::ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                russh::ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                russh::ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                russh::ChannelMsg::ExitSignal { signal_name, .. } => {
                    stderr.extend_from_slice(
                        format!("remote command terminated by signal {signal_name:?}\n").as_bytes(),
                    );
                }
                _ => {}
            }
        }
        let code = code.unwrap_or(0);
        Ok(CmdOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            code,
        })
    }

    pub async fn exec_checked(&self, command: &str) -> Result<CmdOutput> {
        let output = self.exec(command).await?;
        if output.code != 0 {
            let stderr = output.stderr.trim();
            let stdout = output.stdout.trim();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            bail!(
                "remote command failed with exit code {}: {}\n{}",
                output.code,
                command,
                tail(detail, 4000)
            );
        }
        Ok(output)
    }

    pub async fn read_file(&self, path: &str) -> Result<String> {
        let command = format!("test -f {} && cat -- {}", shq(path), shq(path));
        let output = self.exec(&command).await?;
        if output.code != 0 {
            bail!(
                "cannot read remote file {}: {}",
                path,
                tail(output.stderr.trim(), 1000)
            );
        }
        Ok(output.stdout)
    }

    pub async fn sftp(&self) -> Result<SftpSession> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .context("open SFTP channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("request sftp subsystem")?;
        SftpSession::new(channel.into_stream())
            .await
            .context("start SFTP session")
    }

    pub async fn download(&self, remote: &str, local: &Path) -> Result<u64> {
        let sftp = self.sftp().await?;
        let metadata = sftp
            .metadata(remote)
            .await
            .with_context(|| format!("stat remote file {remote}"))?;
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let tmp = local.with_extension(format!(
            "{}.part",
            local.extension().and_then(|e| e.to_str()).unwrap_or("")
        ));
        let mut remote_file = sftp
            .open(remote)
            .await
            .with_context(|| format!("open remote file {remote}"))?;
        let mut local_file = tokio::fs::File::create(&tmp)
            .await
            .with_context(|| format!("create {}", tmp.display()))?;
        tokio::io::copy(&mut remote_file, &mut local_file)
            .await
            .with_context(|| format!("download {remote}"))?;
        local_file.flush().await?;
        local_file.sync_all().await?;
        drop(local_file);
        tokio::fs::rename(&tmp, local)
            .await
            .with_context(|| format!("rename {} to {}", tmp.display(), local.display()))?;
        Ok(metadata.len())
    }
}

fn find_identity(cfg: &ResolvedRemote) -> Option<PathBuf> {
    if let Some(path) = &cfg.identity_file {
        return Some(path.clone());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
        let path = home.join(".ssh").join(name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn key_passphrase() -> Option<String> {
    std::env::var("SSH_KEY_PASSPHRASE")
        .ok()
        .or_else(|| std::env::var("CLONER_SSH_KEY_PASSPHRASE").ok())
        .filter(|s| !s.is_empty())
}

fn tail(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

/// Expand a local identity path for display and loading.
#[allow(dead_code)]
pub fn identity_path(value: &str) -> PathBuf {
    expand_tilde(value)
}
