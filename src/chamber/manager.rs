use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::chamber::Chamber;
use crate::service::job::Job;
use crate::service::job_service::{JobServiceError, validate_cron_expression};

pub use crate::chamber::{ProxyHttpMount, StaticWebMount};

const MAX_TIMEOUT_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// Loads the configured Chambers and retains their runtime Job handles.
pub struct ChamberManager {
    chambers: Vec<Chamber>,
}

#[derive(Clone, Debug)]
pub struct ChamberSummary {
    pub name: String,
    pub job_count: usize,
}

#[derive(Debug)]
pub struct ChamberManagerError(String);

impl fmt::Display for ChamberManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ChamberManagerError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChamberFile {
    chamber: ChamberHeader,
    #[serde(default)]
    jobs: Vec<JobFile>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_staticweb")]
    staticweb: Vec<StaticWebFile>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_proxyhttp")]
    proxyhttp: Vec<ProxyHttpFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChamberHeader {
    name: String,
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobFile {
    name: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    cron: String,
    working_directory: String,
    command: String,
    timeout: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StaticWebFile {
    directory: String,
    route: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyHttpFile {
    #[serde(default)]
    working_directory: Option<String>,
    script: String,
    upstream: String,
    route: String,
}

fn deserialize_staticweb<'de, D>(deserializer: D) -> Result<Vec<StaticWebFile>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StaticWebEntries {
        Multiple(Vec<StaticWebFile>),
        Single(StaticWebFile),
    }

    Ok(match StaticWebEntries::deserialize(deserializer)? {
        StaticWebEntries::Multiple(entries) => entries,
        StaticWebEntries::Single(entry) => vec![entry],
    })
}

fn deserialize_proxyhttp<'de, D>(deserializer: D) -> Result<Vec<ProxyHttpFile>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ProxyHttpEntries {
        Multiple(Vec<ProxyHttpFile>),
        Single(ProxyHttpFile),
    }

    Ok(match ProxyHttpEntries::deserialize(deserializer)? {
        ProxyHttpEntries::Multiple(entries) => entries,
        ProxyHttpEntries::Single(entry) => vec![entry],
    })
}

struct PreparedJob {
    name: String,
    enabled: bool,
    cron: String,
    working_directory: PathBuf,
    command: String,
    timeout: Duration,
}

struct PreparedChamber {
    name: String,
    directory: PathBuf,
    jobs: Vec<PreparedJob>,
    static_web_mounts: Vec<StaticWebMount>,
    proxy_http_mounts: Vec<ProxyHttpMount>,
}

impl ChamberManager {
    /// Loads immediate child directories containing `chamber.toml` in name order.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, ChamberManagerError> {
        let root = root.as_ref();
        fs::create_dir_all(root)
            .map_err(|error| manager_io_error(root, "could not create Chamber root", error))?;
        let entries = fs::read_dir(root)
            .map_err(|error| manager_io_error(root, "could not read Chamber root", error))?;
        let mut entries = entries
            .map(|entry| {
                entry.map_err(|error| {
                    manager_io_error(root, "could not read directory entry", error)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());

        let mut prepared = Vec::new();
        let mut mount_routes = HashSet::new();
        for entry in entries {
            let entry_type = entry.file_type().map_err(|error| {
                manager_io_error(&entry.path(), "could not inspect directory entry", error)
            })?;
            if !entry_type.is_dir() {
                continue;
            }

            let directory = entry.path();
            let config_path = directory.join("chamber.toml");
            let config_metadata = match fs::metadata(&config_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(manager_io_error(
                        &config_path,
                        "could not inspect Chamber configuration",
                        error,
                    ));
                }
            };
            if !config_metadata.is_file() {
                return Err(validation_error(
                    &config_path,
                    "expected chamber.toml to be a file",
                ));
            }

            let directory_name = entry
                .file_name()
                .into_string()
                .map_err(|_| validation_error(&directory, "directory name is not valid UTF-8"))?;
            let source = fs::read_to_string(&config_path).map_err(|error| {
                manager_io_error(&config_path, "could not read Chamber configuration", error)
            })?;
            let config = parse_chamber_file(&source).map_err(|error| {
                validation_error(&config_path, format!("invalid TOML: {error}"))
            })?;

            if config.chamber.name.trim().is_empty() {
                return Err(validation_error(
                    &config_path,
                    "[chamber].name must not be empty",
                ));
            }
            if config.chamber.name != directory_name {
                return Err(validation_error(
                    &config_path,
                    format!(
                        "[chamber].name {:?} must match parent directory {:?}",
                        config.chamber.name, directory_name
                    ),
                ));
            }
            if config.chamber.schema_version != 1 {
                return Err(validation_error(
                    &config_path,
                    format!(
                        "unsupported schema_version {}; expected 1",
                        config.chamber.schema_version
                    ),
                ));
            }

            let mut static_web_mounts = Vec::new();
            for static_web in config.staticweb {
                let route =
                    crate::app::normalize_route_prefix(&static_web.route).map_err(|error| {
                        validation_error(
                            &config_path,
                            format!("staticweb route {:?}: {error}", static_web.route),
                        )
                    })?;
                if !mount_routes.insert(route.clone()) {
                    return Err(validation_error(
                        &config_path,
                        format!("duplicate mount route {route:?}"),
                    ));
                }

                let configured_directory = PathBuf::from(&static_web.directory);
                let directory_path = if configured_directory.is_absolute() {
                    configured_directory
                } else {
                    directory.join(configured_directory)
                };
                let static_directory = fs::canonicalize(&directory_path).map_err(|error| {
                    validation_error(
                        &config_path,
                        format!(
                            "staticweb route {route:?}: could not access directory {}: {error}",
                            directory_path.display()
                        ),
                    )
                })?;
                if !static_directory.is_dir() {
                    return Err(validation_error(
                        &config_path,
                        format!(
                            "staticweb route {route:?}: directory is not a directory: {}",
                            static_directory.display()
                        ),
                    ));
                }

                static_web_mounts.push(StaticWebMount {
                    directory: static_directory,
                    route,
                    config_path: config_path.clone(),
                });
            }

            let mut proxy_http_mounts = Vec::new();
            for proxy in config.proxyhttp {
                let route = crate::app::normalize_route_prefix(&proxy.route).map_err(|error| {
                    validation_error(
                        &config_path,
                        format!("proxyhttp route {:?}: {error}", proxy.route),
                    )
                })?;
                if !mount_routes.insert(route.clone()) {
                    return Err(validation_error(
                        &config_path,
                        format!("duplicate mount route {route:?}"),
                    ));
                }
                if proxy.script.trim().is_empty() {
                    return Err(validation_error(
                        &config_path,
                        format!("proxyhttp route {route:?}: script must not be empty"),
                    ));
                }
                crate::proxy_http::validate_upstream(&proxy.upstream).map_err(|error| {
                    validation_error(
                        &config_path,
                        format!("proxyhttp route {route:?}: invalid upstream: {error}"),
                    )
                })?;

                let configured_directory =
                    PathBuf::from(proxy.working_directory.as_deref().unwrap_or("."));
                let directory_path = if configured_directory.is_absolute() {
                    configured_directory
                } else {
                    directory.join(configured_directory)
                };
                let working_directory = fs::canonicalize(&directory_path).map_err(|error| {
                    validation_error(
                        &config_path,
                        format!(
                            "proxyhttp route {route:?}: could not access working_directory {}: {error}",
                            directory_path.display()
                        ),
                    )
                })?;
                if !working_directory.is_dir() {
                    return Err(validation_error(
                        &config_path,
                        format!(
                            "proxyhttp route {route:?}: working_directory is not a directory: {}",
                            working_directory.display()
                        ),
                    ));
                }

                proxy_http_mounts.push(ProxyHttpMount {
                    working_directory,
                    script: proxy.script,
                    upstream: proxy.upstream,
                    route,
                    config_path: config_path.clone(),
                });
            }

            let mut names = HashSet::new();
            let mut jobs = Vec::with_capacity(config.jobs.len());
            for job in config.jobs {
                if job.name.trim().is_empty() {
                    return Err(validation_error(
                        &config_path,
                        "each [[jobs]] entry must have a non-empty name",
                    ));
                }
                if !names.insert(job.name.clone()) {
                    return Err(validation_error(
                        &config_path,
                        format!("duplicate job name {:?}", job.name),
                    ));
                }

                if job.cron.split_whitespace().count() != 5 {
                    return Err(validation_error(
                        &config_path,
                        format!("job {:?}: cron must have exactly five fields", job.name),
                    ));
                }
                validate_cron_expression(&job.cron).map_err(|error| {
                    validation_error(&config_path, format!("job {:?}: {error}", job.name))
                })?;

                let timeout = parse_timeout(&job.timeout).map_err(|message| {
                    validation_error(
                        &config_path,
                        format!("job {:?}: invalid timeout: {message}", job.name),
                    )
                })?;

                let configured_working_directory = PathBuf::from(&job.working_directory);
                let working_directory = if configured_working_directory.is_absolute() {
                    configured_working_directory
                } else {
                    directory.join(configured_working_directory)
                };
                match fs::metadata(&working_directory) {
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => {
                        return Err(validation_error(
                            &config_path,
                            format!(
                                "job {:?}: working_directory is not a directory: {}",
                                job.name,
                                working_directory.display()
                            ),
                        ));
                    }
                    Err(error) => {
                        return Err(validation_error(
                            &config_path,
                            format!(
                                "job {:?}: could not access working_directory {}: {error}",
                                job.name,
                                working_directory.display()
                            ),
                        ));
                    }
                }

                jobs.push(PreparedJob {
                    name: job.name,
                    enabled: job.enabled,
                    cron: job.cron,
                    working_directory,
                    command: job.command,
                    timeout,
                });
            }

            prepared.push(PreparedChamber {
                name: config.chamber.name,
                directory,
                jobs,
                static_web_mounts,
                proxy_http_mounts,
            });
        }

        let mut chambers = Vec::with_capacity(prepared.len());
        for prepared_chamber in prepared {
            let chamber = Chamber::from_config(
                prepared_chamber.name,
                prepared_chamber.directory.clone(),
                prepared_chamber.static_web_mounts,
                prepared_chamber.proxy_http_mounts,
            );
            for job in prepared_chamber.jobs {
                chamber
                    .create_job_with_enabled(
                        job.name,
                        job.cron,
                        job.working_directory,
                        job.command,
                        job.timeout,
                        job.enabled,
                    )
                    .map_err(|error| {
                        validation_error(
                            prepared_chamber.directory.join("chamber.toml"),
                            format!("could not start job worker: {error}"),
                        )
                    })?;
            }
            chambers.push(chamber);
        }

        Ok(Self { chambers })
    }

    pub fn chambers(&self) -> &[Chamber] {
        &self.chambers
    }

    pub fn static_web_mounts(&self) -> impl Iterator<Item = &StaticWebMount> {
        self.chambers.iter().flat_map(Chamber::static_web_mounts)
    }

    pub fn proxy_http_mounts(&self) -> impl Iterator<Item = &ProxyHttpMount> {
        self.chambers.iter().flat_map(Chamber::proxy_http_mounts)
    }

    pub fn summaries(&self) -> Vec<ChamberSummary> {
        self.chambers
            .iter()
            .map(|chamber| ChamberSummary {
                name: chamber.name.clone(),
                job_count: chamber.jobs().len(),
            })
            .collect()
    }

    pub fn jobs_for_chamber(&self, name: &str) -> Option<Vec<Job>> {
        self.chambers
            .iter()
            .find(|chamber| chamber.name == name)
            .map(Chamber::jobs)
    }

    pub fn list_jobs(&self) -> Vec<(String, Job)> {
        let mut jobs = self
            .chambers
            .iter()
            .flat_map(|chamber| {
                chamber
                    .jobs()
                    .into_iter()
                    .map(|job| (chamber.name.clone(), job))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        jobs.sort_by_key(|(_, job)| job.id());
        jobs
    }

    pub fn pause_job(&self, id: u64) -> Result<Job, JobServiceError> {
        for chamber in &self.chambers {
            if chamber.jobs().iter().any(|job| job.id() == id) {
                return chamber.pause_job(id);
            }
        }
        Err(JobServiceError::NotFound(id))
    }

    pub fn start_job(&self, id: u64) -> Result<Job, JobServiceError> {
        for chamber in &self.chambers {
            if chamber.jobs().iter().any(|job| job.id() == id) {
                return chamber.start_job(id);
            }
        }
        Err(JobServiceError::NotFound(id))
    }

    pub fn chamber_directory(&self, name: &str) -> Option<&Path> {
        self.chambers
            .iter()
            .find(|chamber| chamber.name == name)
            .map(|chamber| chamber.directory.as_path())
    }
}

fn default_enabled() -> bool {
    true
}

fn parse_chamber_file(source: &str) -> Result<ChamberFile, toml::de::Error> {
    toml::from_str(source)
}

fn parse_timeout(value: &str) -> Result<Duration, String> {
    let (digits, multiplier) = if let Some(digits) = value.strip_suffix("ms") {
        (digits, 1_u64)
    } else if let Some(digits) = value.strip_suffix('s') {
        (digits, 1_000)
    } else if let Some(digits) = value.strip_suffix('m') {
        (digits, 60_000)
    } else if let Some(digits) = value.strip_suffix('h') {
        (digits, 3_600_000)
    } else {
        return Err("expected an integer followed by ms, s, m, or h".into());
    };

    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected a positive integer followed by a unit".into());
    }
    let amount = digits
        .parse::<u64>()
        .map_err(|_| "integer is out of range".to_owned())?;
    let milliseconds = amount
        .checked_mul(multiplier)
        .ok_or_else(|| "duration is out of range".to_owned())?;
    if milliseconds == 0 {
        return Err("duration must be greater than zero".into());
    }
    if milliseconds > MAX_TIMEOUT_MILLIS {
        return Err("duration must not exceed 24 hours".into());
    }

    Ok(Duration::from_millis(milliseconds))
}

fn manager_io_error(
    path: impl AsRef<Path>,
    action: &str,
    error: std::io::Error,
) -> ChamberManagerError {
    ChamberManagerError(format!("{}: {action}: {error}", path.as_ref().display()))
}

fn validation_error(path: impl AsRef<Path>, message: impl fmt::Display) -> ChamberManagerError {
    ChamberManagerError(format!("{}: {message}", path.as_ref().display()))
}

#[cfg(test)]
#[path = "tests/manager.rs"]
mod tests;
