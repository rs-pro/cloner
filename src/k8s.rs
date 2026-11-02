//! Kubernetes execution and backup manifests.
//!
//! Two independent pieces:
//!
//! * [`KubernetesTarget`] wraps a database command in `kubectl exec`, so an app
//!   running in a cluster can be dumped/restored the same way as a Compose
//!   service.
//! * [`BackupCronJob`] renders a nightly `pg_dump` + upload CronJob. The dump
//!   runs in an init container and uploads straight to object storage; nothing
//!   is mounted with CSI and the upload key never needs delete permission.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::engine::{Engine, ResolvedDb};
use crate::shell::shq;

/// How to reach the pod that runs the database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KubernetesTarget {
    pub context: Option<String>,
    pub namespace: Option<String>,
    /// Explicit pod name. When empty, `selector` is used to resolve one.
    pub pod: Option<String>,
    /// Label selector (`app.kubernetes.io/name=postgres`) for pod resolution.
    pub selector: Option<String>,
    pub container: Option<String>,
}

impl KubernetesTarget {
    pub fn validate(&self) -> Result<&Self> {
        if self.pod.as_deref().unwrap_or("").trim().is_empty()
            && self.selector.as_deref().unwrap_or("").trim().is_empty()
        {
            bail!("kubernetes.pod or kubernetes.selector is required");
        }
        Ok(self)
    }

    /// Common flags for every kubectl invocation.
    pub fn kubectl_flags(&self) -> Vec<String> {
        let mut parts = vec!["kubectl".to_string()];
        if let Some(context) = self.context.as_deref().filter(|v| !v.is_empty()) {
            parts.push("--context".to_string());
            parts.push(shq(context));
        }
        if let Some(namespace) = self.namespace.as_deref().filter(|v| !v.is_empty()) {
            parts.push("--namespace".to_string());
            parts.push(shq(namespace));
        }
        parts
    }

    /// Command that prints the name of a matching pod (`kubectl get pods`).
    pub fn resolve_pod_command(&self) -> Result<String> {
        let selector = self
            .selector
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("kubernetes.selector is required to resolve a pod"))?;
        Ok(format!(
            "{} get pods -l {} --field-selector=status.phase=Running \
             -o jsonpath='{{.items[0].metadata.name}}'",
            self.kubectl_flags().join(" "),
            shq(selector)
        ))
    }

    /// Wrap `inner` in `kubectl exec`. `stdin` adds `-i`.
    pub fn wrap(&self, inner: &str, stdin: bool) -> Result<String> {
        let pod = self.pod.as_deref().unwrap_or("").trim();
        if pod.is_empty() {
            bail!("kubernetes.pod is not set; resolve the pod first");
        }
        let mut parts = self.kubectl_flags();
        parts.push("exec".to_string());
        if stdin {
            parts.push("-i".to_string());
        }
        if let Some(container) = self.container.as_deref().filter(|v| !v.is_empty()) {
            parts.push("--container".to_string());
            parts.push(shq(container));
        }
        parts.push(shq(pod));
        parts.push("--".to_string());
        parts.push("sh".to_string());
        parts.push("-lc".to_string());
        parts.push(shq(inner));
        Ok(parts.join(" "))
    }
}

/// Config schema for `local.kubernetes` / `remote.kubernetes`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KubernetesConfig {
    pub context: Option<String>,
    pub namespace: Option<String>,
    pub pod: Option<String>,
    pub selector: Option<String>,
    pub container: Option<String>,
}

impl From<KubernetesConfig> for KubernetesTarget {
    fn from(value: KubernetesConfig) -> Self {
        Self {
            context: value.context.clone(),
            namespace: value.namespace.clone(),
            pod: value.pod.clone(),
            selector: value.selector.clone(),
            container: value.container.clone(),
        }
    }
}

impl From<&KubernetesConfig> for KubernetesTarget {
    fn from(value: &KubernetesConfig) -> Self {
        value.clone().into()
    }
}

// ---------------------------------------------------------------------------
// Nightly dump CronJob

#[derive(Clone, Debug)]
pub struct BackupCronJob {
    pub name: String,
    pub schedule: String,
    pub time_zone: String,
    pub database: ResolvedDb,
    /// `rclone` destination, e.g. `yc:blog-db-backups/postgres`.
    pub bucket: String,
    /// rclone remote name; becomes `RCLONE_CONFIG_<NAME>_*`.
    pub remote_name: String,
    pub s3_endpoint: String,
    pub s3_region: String,
    pub secret_name: String,
    pub dump_image: String,
    pub upload_image: String,
    /// `emptyDir` limit, e.g. `512Mi`.
    pub size_limit: String,
    pub compress: String,
    /// Set to 0 to disable `successfulJobsHistoryLimit`.
    pub successful_jobs_history: u32,
    pub failed_jobs_history: u32,
    pub security_uid: u32,
    pub security_gid: u32,
}

impl BackupCronJob {
    pub fn for_database(
        name: impl Into<String>,
        database: ResolvedDb,
        bucket: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            schedule: "30 3 * * *".to_string(),
            time_zone: "Europe/Minsk".to_string(),
            database,
            bucket: bucket.into(),
            remote_name: "yc".to_string(),
            s3_endpoint: "https://storage.yandexcloud.net".to_string(),
            s3_region: "ru-central1".to_string(),
            secret_name: "db-backup".to_string(),
            dump_image: "postgres:18-alpine".to_string(),
            upload_image: "rclone/rclone:1.75.1".to_string(),
            size_limit: "512Mi".to_string(),
            compress: "6".to_string(),
            successful_jobs_history: 7,
            failed_jobs_history: 3,
            security_uid: 70,
            security_gid: 70,
        }
    }

    /// Render the manifest as YAML.
    pub fn render(&self) -> Result<String> {
        if self.database.engine != Engine::Postgres {
            bail!("the CronJob template currently supports PostgreSQL only");
        }
        if self.database.database.is_empty() {
            bail!("database name is required for the backup CronJob");
        }
        if self.database.user.is_empty() {
            bail!("database user is required for the backup CronJob");
        }
        if self.bucket.trim().is_empty() {
            bail!("an object-storage destination is required (for example yc:bucket/postgres)");
        }

        let remote = self.remote_name.to_ascii_uppercase();
        let password_key = "PGPASSWORD";
        let access_key = "S3_ACCESS_KEY";
        let secret_key = "S3_SECRET_KEY";

        let dump_script = format!(
            "ts=\"$(date +%Y%m%d-%H%M%S)\"\n\
             out=\"/backups/${{PGDATABASE}}-${{ts}}.dump\"\n\
             pg_dump --format=custom --compress={} --file=\"${{out}}\"\n\
             [ -s \"${{out}}\" ]\n\
             pg_restore --list \"${{out}}\" > /dev/null\n\
             ls -lh /backups",
            self.compress
        );

        let manifest = json!({
            "apiVersion": "batch/v1",
            "kind": "CronJob",
            "metadata": {"name": self.name},
            "spec": {
                "schedule": self.schedule,
                "timeZone": self.time_zone,
                "concurrencyPolicy": "Forbid",
                "successfulJobsHistoryLimit": self.successful_jobs_history,
                "failedJobsHistoryLimit": self.failed_jobs_history,
                "jobTemplate": {
                    "spec": {
                        "backoffLimit": 2,
                        "activeDeadlineSeconds": 1800,
                        "template": {
                            "spec": {
                                "restartPolicy": "Never",
                                "securityContext": {
                                    "runAsNonRoot": true,
                                    "runAsUser": self.security_uid,
                                    "runAsGroup": self.security_gid,
                                    "fsGroup": self.security_gid,
                                },
                                "initContainers": [{
                                    "name": "dump",
                                    "image": self.dump_image,
                                    "env": [
                                        {"name": "PGHOST", "value": self.database.host},
                                        {"name": "PGPORT", "value": self.database.port.to_string()},
                                        {"name": "PGDATABASE", "value": self.database.database},
                                        {"name": "PGUSER", "value": self.database.user},
                                        {"name": "PGPASSWORD", "valueFrom": {"secretKeyRef": {"name": self.secret_name, "key": password_key}}},
                                    ],
                                    "command": ["sh", "-ec", dump_script],
                                    "volumeMounts": [{"name": "backups", "mountPath": "/backups"}],
                                }],
                                "containers": [{
                                    "name": "upload",
                                    "image": self.upload_image,
                                    "env": [
                                        {"name": format!("RCLONE_CONFIG_{remote}_TYPE"), "value": "s3"},
                                        {"name": format!("RCLONE_CONFIG_{remote}_PROVIDER"), "value": "Other"},
                                        {"name": format!("RCLONE_CONFIG_{remote}_ENDPOINT"), "value": self.s3_endpoint},
                                        {"name": format!("RCLONE_CONFIG_{remote}_REGION"), "value": self.s3_region},
                                        {"name": format!("RCLONE_CONFIG_{remote}_ACCESS_KEY_ID"), "valueFrom": {"secretKeyRef": {"name": self.secret_name, "key": access_key}}},
                                        {"name": format!("RCLONE_CONFIG_{remote}_SECRET_ACCESS_KEY"), "valueFrom": {"secretKeyRef": {"name": self.secret_name, "key": secret_key}}},
                                        {"name": "S3_DEST", "value": self.bucket},
                                    ],
                                    "args": [
                                        "copy",
                                        "/backups",
                                        "$(S3_DEST)",
                                        "--s3-no-check-bucket",
                                        "--s3-no-head",
                                        "--no-check-dest",
                                        "-v",
                                    ],
                                    "volumeMounts": [{"name": "backups", "mountPath": "/backups", "readOnly": true}],
                                }],
                                "volumes": [{
                                    "name": "backups",
                                    "emptyDir": {"sizeLimit": self.size_limit},
                                }],
                            }
                        }
                    }
                }
            }
        });

        serde_yaml::to_string(&manifest).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> KubernetesTarget {
        KubernetesTarget {
            context: Some("prod".into()),
            namespace: Some("app".into()),
            pod: Some("postgres-0".into()),
            selector: None,
            container: Some("postgres".into()),
        }
    }

    #[test]
    fn wraps_commands_in_kubectl_exec() {
        let command = target().wrap("pg_dump -Fc app", false).unwrap();
        assert!(command.starts_with("kubectl --context prod --namespace app exec"));
        assert!(command.contains("--container postgres"));
        assert!(command.contains("postgres-0 -- sh -lc 'pg_dump -Fc app'"));
        assert!(!command.contains(" -i "));

        let with_stdin = target().wrap("pg_restore", true).unwrap();
        assert!(with_stdin.contains("exec -i"));
    }

    #[test]
    fn builds_a_pod_resolution_command() {
        let selector = KubernetesTarget {
            namespace: Some("app".into()),
            selector: Some("app.kubernetes.io/name=postgres".into()),
            ..KubernetesTarget::default()
        };
        let command = selector.resolve_pod_command().unwrap();
        assert!(command.contains("get pods -l"));
        assert!(command.contains("status.phase=Running"));
        assert!(command.contains("jsonpath"));
        assert!(selector.validate().is_ok());

        let empty = KubernetesTarget::default();
        assert!(empty.validate().is_err());
        assert!(empty.wrap("true", false).is_err());
    }

    #[test]
    fn renders_a_nightly_postgres_cronjob() {
        let db = ResolvedDb {
            engine: Engine::Postgres,
            host: "pg.example.com".into(),
            port: 5432,
            database: "blog".into(),
            user: "blog".into(),
            password: "unused".into(),
            service: None,
            in_container: false,
        };
        let job = BackupCronJob::for_database("blog-backup", db, "yc:blog-db-backups/postgres");
        let yaml = job.render().unwrap();
        assert!(yaml.contains("kind: CronJob"));
        assert!(yaml.contains("schedule: 30 3 * * *"));
        assert!(yaml.contains("timeZone: Europe/Minsk"));
        assert!(yaml.contains("concurrencyPolicy: Forbid"));
        assert!(yaml.contains("name: dump"));
        assert!(yaml.contains("pg_dump --format=custom --compress=6"));
        assert!(yaml.contains("pg_restore --list"));
        assert!(yaml.contains("RCLONE_CONFIG_YC_TYPE"));
        assert!(yaml.contains("--s3-no-check-bucket"));
        assert!(yaml.contains("S3_DEST"));
        assert!(yaml.contains("emptyDir"));
        assert!(yaml.contains("sizeLimit: 512Mi"));
        assert!(yaml.contains("secretKeyRef"));
    }

    #[test]
    fn rejects_unsupported_cronjob_databases() {
        let db = ResolvedDb {
            engine: Engine::Mysql,
            host: "localhost".into(),
            port: 3306,
            database: "app".into(),
            user: "app".into(),
            password: String::new(),
            service: None,
            in_container: false,
        };
        let job = BackupCronJob::for_database("app", db, "yc:app");
        assert!(job.render().unwrap_err().to_string().contains("PostgreSQL"));
    }
}
