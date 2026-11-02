//! Where a command runs: the host, a Compose service, or a Kubernetes pod.

use anyhow::Result;

use crate::k8s::KubernetesTarget;
use crate::shell::shq;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CommandTarget {
    #[default]
    Host,
    Compose {
        path: String,
        file: String,
    },
    Kubernetes(KubernetesTarget),
}

impl CommandTarget {
    pub fn compose(path: impl Into<String>, file: impl Into<String>) -> Self {
        Self::Compose {
            path: path.into(),
            file: file.into(),
        }
    }

    pub fn kubernetes(target: KubernetesTarget) -> Self {
        Self::Kubernetes(target)
    }

    pub fn is_host(&self) -> bool {
        matches!(self, Self::Host)
    }

    /// Wrap a database command so it runs in the selected container.
    ///
    /// `service` is the Compose service name; it is required for Compose and
    /// ignored for Kubernetes (the pod is part of the target).
    pub fn wrap(&self, service: Option<&str>, inner: &str) -> Result<String> {
        match self {
            Self::Host => Ok(inner.to_string()),
            Self::Compose { path, file } => {
                let service = service.filter(|s| !s.is_empty()).ok_or_else(|| {
                    anyhow::anyhow!("a Compose service is required to run this command")
                })?;
                Ok(format!(
                    "cd {} && docker compose -f {} exec -T {} {}",
                    shq(path),
                    shq(file),
                    shq(service),
                    inner
                ))
            }
            Self::Kubernetes(target) => target.wrap(inner, false),
        }
    }

    /// Like [`Self::wrap`] but keeps stdin open for `restore < dump`.
    pub fn wrap_stdin(&self, service: Option<&str>, inner: &str) -> Result<String> {
        match self {
            Self::Kubernetes(target) => target.wrap(inner, true),
            other => other.wrap(service, inner),
        }
    }

    /// `docker compose ...` prefix for lifecycle commands (`up`, `down`).
    pub fn compose_prefix(&self) -> Option<String> {
        match self {
            Self::Compose { path, file } => Some(format!(
                "cd {} && docker compose -f {}",
                shq(path),
                shq(file)
            )),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Compose { .. } => "compose",
            Self::Kubernetes(_) => "kubernetes",
        }
    }

    /// Validate that the target has everything needed to run commands.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Host => Ok(()),
            Self::Compose { .. } => Ok(()),
            Self::Kubernetes(target) => {
                target.validate()?;
                Ok(())
            }
        }
    }
}

/// `cd <path> && docker compose -f <file>` for a resolved Compose project.
pub fn compose_prefix(path: &str, file: &str) -> String {
    format!("cd {} && docker compose -f {}", shq(path), shq(file))
}

impl crate::config::ResolvedLocal {
    /// Command target for local database/file operations.
    pub fn command_target(&self) -> CommandTarget {
        if let Some(kubernetes) = &self.kubernetes {
            return CommandTarget::kubernetes(kubernetes.clone().into());
        }
        match &self.compose {
            Some(compose) => CommandTarget::compose(&compose.path, &compose.file),
            None => CommandTarget::Host,
        }
    }
}

impl crate::config::ResolvedRemote {
    /// Command target for remote database/file operations.
    pub fn command_target(&self) -> CommandTarget {
        if let Some(kubernetes) = &self.kubernetes {
            return CommandTarget::kubernetes(kubernetes.clone().into());
        }
        match &self.compose {
            Some(compose) => CommandTarget::compose(&compose.path, &compose.file),
            None => CommandTarget::Host,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_target_returns_the_command_unchanged() {
        let target = CommandTarget::Host;
        assert!(target.is_host());
        assert_eq!(target.wrap(None, "pg_dump app").unwrap(), "pg_dump app");
        assert_eq!(target.kind(), "host");
    }

    #[test]
    fn compose_target_wraps_with_quoting() {
        let target = CommandTarget::compose("/srv/my app", "compose.yml");
        let command = target.wrap(Some("db"), "pg_dump app").unwrap();
        assert_eq!(
            command,
            "cd '/srv/my app' && docker compose -f compose.yml exec -T db pg_dump app"
        );
        assert_eq!(
            target.compose_prefix().unwrap(),
            "cd '/srv/my app' && docker compose -f compose.yml"
        );
        assert!(target.wrap(None, "true").is_err());
        assert!(target.wrap(Some(""), "true").is_err());
    }

    #[test]
    fn kubernetes_target_keeps_stdin_open_for_restores() {
        let target = CommandTarget::kubernetes(KubernetesTarget {
            context: None,
            namespace: Some("app".into()),
            pod: Some("pg-0".into()),
            selector: None,
            container: None,
        });
        let plain = target.wrap(Some("ignored"), "pg_dump").unwrap();
        assert_eq!(plain, "kubectl --namespace app exec pg-0 -- sh -lc pg_dump");
        let stdin = target.wrap_stdin(None, "pg_restore").unwrap();
        assert!(stdin.contains("exec -i pg-0"));
        assert_eq!(target.kind(), "kubernetes");
    }

    #[test]
    fn missing_kubernetes_pod_is_an_error() {
        let target = CommandTarget::kubernetes(KubernetesTarget::default());
        assert!(target.validate().is_err());
        assert!(target.wrap(None, "true").is_err());
    }
}
