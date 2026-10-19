use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::service::job::Job;
use crate::service::job_service::{JobService, JobServiceError};

pub mod manager;
pub use manager::ChamberManager;

#[derive(Clone, Debug)]
pub struct StaticWebMount {
    pub directory: PathBuf,
    pub route: String,
    pub config_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ProxyHttpMount {
    pub working_directory: PathBuf,
    pub script: String,
    pub upstream: String,
    pub route: String,
    pub config_path: PathBuf,
}

/// A configured Chamber owns its identity, mount configuration, and runtime Jobs.
pub struct Chamber {
    pub name: String,
    pub directory: PathBuf,
    job_service: &'static JobService,
    jobs: Mutex<Vec<Job>>,
    pub static_web: Vec<StaticWebMount>,
    pub proxy_http: Vec<ProxyHttpMount>,
}

impl Chamber {
    /// Creates an in-memory Chamber with no configured mounts.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            directory: PathBuf::from("."),
            job_service: JobService::global(),
            jobs: Mutex::new(Vec::new()),
            static_web: Vec::new(),
            proxy_http: Vec::new(),
        }
    }

    pub(crate) fn from_config(
        name: String,
        directory: PathBuf,
        static_web: Vec<StaticWebMount>,
        proxy_http: Vec<ProxyHttpMount>,
    ) -> Self {
        Self {
            name,
            directory,
            job_service: JobService::global(),
            jobs: Mutex::new(Vec::new()),
            static_web,
            proxy_http,
        }
    }

    pub fn static_web_mounts(&self) -> &[StaticWebMount] {
        &self.static_web
    }

    pub fn proxy_http_mounts(&self) -> &[ProxyHttpMount] {
        &self.proxy_http
    }

    pub fn create_job(
        &self,
        name: impl Into<String>,
        cron_expression: impl Into<String>,
        working_directory: impl AsRef<Path>,
        shell_command: impl Into<String>,
        timeout: Duration,
    ) -> Result<Job, JobServiceError> {
        self.create_job_with_enabled(
            name,
            cron_expression,
            working_directory,
            shell_command,
            timeout,
            true,
        )
    }

    pub(crate) fn create_job_with_enabled(
        &self,
        name: impl Into<String>,
        cron_expression: impl Into<String>,
        working_directory: impl AsRef<Path>,
        shell_command: impl Into<String>,
        timeout: Duration,
        enabled: bool,
    ) -> Result<Job, JobServiceError> {
        let job = self.job_service.create_job_with_enabled(
            name,
            cron_expression,
            working_directory,
            shell_command,
            timeout,
            enabled,
        )?;
        self.jobs
            .lock()
            .expect("chamber job list lock poisoned")
            .push(job.clone());
        Ok(job)
    }

    pub fn pause_job(&self, id: u64) -> Result<Job, JobServiceError> {
        self.ensure_contains_job(id)?;
        self.job_service.pause_job(id)
    }

    pub fn start_job(&self, id: u64) -> Result<Job, JobServiceError> {
        self.ensure_contains_job(id)?;
        self.job_service.start_job(id)
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.jobs
            .lock()
            .expect("chamber job list lock poisoned")
            .clone()
    }

    pub fn job_service(&self) -> &'static JobService {
        self.job_service
    }

    fn ensure_contains_job(&self, id: u64) -> Result<(), JobServiceError> {
        let jobs = self.jobs.lock().expect("chamber job list lock poisoned");
        if jobs.iter().any(|job| job.id() == id) {
            Ok(())
        } else {
            Err(JobServiceError::NotFound(id))
        }
    }
}
