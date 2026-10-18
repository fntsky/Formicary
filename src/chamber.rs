use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::job::Job;
use crate::job_service::{JobService, JobServiceError};

/// A chamber references the global job service and keeps handles to its jobs.
pub struct Chamber {
    job_service: &'static JobService,
    jobs: Mutex<Vec<Job>>,
}

impl Chamber {
    pub fn new() -> Self {
        Self {
            job_service: JobService::global(),
            jobs: Mutex::new(Vec::new()),
        }
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

impl Default for Chamber {
    fn default() -> Self {
        Self::new()
    }
}
