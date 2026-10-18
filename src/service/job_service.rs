use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Local};
use cron::Schedule;

use crate::service::job::{Job, JobSnapshot};
use crate::service::shell_runner::{ShellRunner, ShellRunnerError};

static GLOBAL_JOB_SERVICE: OnceLock<JobService> = OnceLock::new();

/// Process-wide manager for scheduled jobs. Pause and start state belongs to
/// each `Job`, not to this service.
pub struct JobService {
    jobs: Mutex<HashMap<u64, ManagedJob>>,
    next_id: AtomicU64,
    shutting_down: AtomicBool,
}

struct ManagedJob {
    control: JobControl,
    worker: Option<JoinHandle<()>>,
}

type JobControl = Arc<(Mutex<JobState>, Condvar)>;

struct JobState {
    job: Job,
    schedule: Schedule,
    revision: u64,
    stopped: bool,
    last_scheduled: Option<DateTime<Local>>,
}

#[derive(Debug)]
pub enum JobServiceError {
    InvalidCron(String),
    NotFound(u64),
    ShuttingDown,
    WorkerSpawn(io::Error),
}

impl fmt::Display for JobServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCron(expression) => {
                write!(f, "invalid cron expression: {expression}")
            }
            Self::NotFound(id) => write!(f, "job {id} was not found"),
            Self::ShuttingDown => write!(f, "job service is shutting down"),
            Self::WorkerSpawn(error) => write!(f, "could not start job worker: {error}"),
        }
    }
}

impl Error for JobServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::WorkerSpawn(error) => Some(error),
            _ => None,
        }
    }
}

impl JobService {
    /// Returns the single process-wide job service instance.
    pub fn global() -> &'static Self {
        GLOBAL_JOB_SERVICE.get_or_init(Self::new)
    }

    fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            shutting_down: AtomicBool::new(false),
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
        let cron_expression = cron_expression.into();
        let schedule = parse_schedule(&cron_expression)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let job = Job::new(JobSnapshot {
            id,
            name: name.into(),
            cron_expression,
            working_directory: working_directory.as_ref().to_path_buf(),
            shell_command: shell_command.into(),
            timeout,
            enabled,
        });
        let control = Arc::new((
            Mutex::new(JobState {
                job: job.clone(),
                schedule,
                revision: 0,
                stopped: false,
                last_scheduled: None,
            }),
            Condvar::new(),
        ));

        let mut jobs = self.jobs.lock().expect("job service lock poisoned");
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(JobServiceError::ShuttingDown);
        }

        let worker_control = Arc::clone(&control);
        let worker = thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || run_job(worker_control))
            .map_err(JobServiceError::WorkerSpawn)?;

        jobs.insert(
            id,
            ManagedJob {
                control,
                worker: Some(worker),
            },
        );
        Ok(job)
    }

    /// Updates a job's schedule and shell configuration, then wakes its worker
    /// so the new schedule takes effect immediately.
    pub fn update_job(
        &self,
        id: u64,
        name: impl Into<String>,
        cron_expression: impl Into<String>,
        working_directory: impl AsRef<Path>,
        shell_command: impl Into<String>,
        timeout: Duration,
    ) -> Result<Job, JobServiceError> {
        let cron_expression = cron_expression.into();
        let schedule = parse_schedule(&cron_expression)?;
        let jobs = self.jobs.lock().expect("job service lock poisoned");
        let managed = jobs.get(&id).ok_or(JobServiceError::NotFound(id))?;
        let (state_lock, wake) = &*managed.control;
        let mut state = state_lock.lock().expect("job state lock poisoned");
        state.job.update(
            name.into(),
            cron_expression,
            working_directory.as_ref().to_path_buf(),
            shell_command.into(),
            timeout,
        );
        state.schedule = schedule;
        state.last_scheduled = None;
        state.revision = state.revision.wrapping_add(1);
        let updated_job = state.job.clone();
        wake.notify_one();
        Ok(updated_job)
    }

    pub fn pause_job(&self, id: u64) -> Result<Job, JobServiceError> {
        self.set_job_enabled(id, false)
    }

    pub fn start_job(&self, id: u64) -> Result<Job, JobServiceError> {
        self.set_job_enabled(id, true)
    }

    pub fn get_job(&self, id: u64) -> Option<Job> {
        let jobs = self.jobs.lock().expect("job service lock poisoned");
        let managed = jobs.get(&id)?;
        let (state, _) = &*managed.control;
        Some(state.lock().expect("job state lock poisoned").job.clone())
    }

    pub fn list_jobs(&self) -> Vec<Job> {
        let jobs = self.jobs.lock().expect("job service lock poisoned");
        let mut jobs = jobs
            .values()
            .map(|managed| {
                let (state, _) = &*managed.control;
                state.lock().expect("job state lock poisoned").job.clone()
            })
            .collect::<Vec<_>>();
        jobs.sort_by_key(Job::id);
        jobs
    }

    /// Stops the workers and waits for any currently running shell commands to
    /// finish. This is a service lifecycle operation, not a job pause state.
    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }

        let mut jobs = self.jobs.lock().expect("job service lock poisoned");
        let managed_jobs = jobs.drain().map(|(_, managed)| managed).collect::<Vec<_>>();
        drop(jobs);

        for managed in &managed_jobs {
            let (state, wake) = &*managed.control;
            state.lock().expect("job state lock poisoned").stopped = true;
            wake.notify_one();
        }

        for mut managed in managed_jobs {
            if let Some(worker) = managed.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn set_job_enabled(&self, id: u64, enabled: bool) -> Result<Job, JobServiceError> {
        let jobs = self.jobs.lock().expect("job service lock poisoned");
        let managed = jobs.get(&id).ok_or(JobServiceError::NotFound(id))?;
        let (state_lock, wake) = &*managed.control;
        let mut state = state_lock.lock().expect("job state lock poisoned");
        state.job.set_enabled(enabled);
        state.revision = state.revision.wrapping_add(1);
        let job = state.job.clone();
        wake.notify_one();
        Ok(job)
    }
}

impl Drop for JobService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn parse_schedule(expression: &str) -> Result<Schedule, JobServiceError> {
    // Accept standard five-field Unix cron expressions as well as cron crate's
    // six- and seven-field forms (which include seconds and optional year).
    let normalized = match expression.split_whitespace().count() {
        5 => format!("0 {expression} *"),
        6 => format!("{expression} *"),
        _ => expression.to_owned(),
    };
    Schedule::from_str(&normalized).map_err(|_| JobServiceError::InvalidCron(expression.into()))
}

pub(crate) fn validate_cron_expression(expression: &str) -> Result<(), JobServiceError> {
    parse_schedule(expression).map(|_| ())
}

fn run_job(control: JobControl) {
    loop {
        let (lock, wake) = &*control;
        let state = lock.lock().expect("job state lock poisoned");
        if state.stopped {
            return;
        }

        let now = Local::now();
        let next_run = state
            .schedule
            .upcoming(Local)
            .find(|scheduled| state.last_scheduled.is_none_or(|last| *scheduled > last));
        let Some(next_run) = next_run else {
            let _ = wake
                .wait_timeout(state, Duration::from_secs(60))
                .expect("job state lock poisoned");
            continue;
        };

        let wait = next_run
            .signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO);
        let revision = state.revision;
        let (mut state, wait_result) = wake
            .wait_timeout_while(state, wait, |state| {
                !state.stopped && state.revision == revision
            })
            .expect("job state lock poisoned");

        if state.stopped {
            return;
        }
        if state.revision != revision || !wait_result.timed_out() {
            continue;
        }
        if Local::now() < next_run {
            continue;
        }

        state.last_scheduled = Some(next_run);
        let job = state.job.snapshot();
        if !job.enabled {
            continue;
        }
        drop(state);

        run_shell_job(&job);
    }
}

fn run_shell_job(job: &JobSnapshot) {
    println!("[job {}] running: {}", job.id, job.shell_command);
    match ShellRunner::run_with_timeout(&job.working_directory, &job.shell_command, job.timeout) {
        Ok(output) => {
            if !output.stdout.is_empty() {
                print!(
                    "[job {} stdout] {}",
                    job.id,
                    String::from_utf8_lossy(&output.stdout)
                );
            }
            if !output.stderr.is_empty() {
                eprint!(
                    "[job {} stderr] {}",
                    job.id,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if !output.status.success() {
                eprintln!("[job {}] exited with status {}", job.id, output.status);
            }
        }
        Err(ShellRunnerError::TimedOut { timeout }) => {
            eprintln!("[job {}] timed out after {timeout:?}", job.id)
        }
        Err(ShellRunnerError::Io(error)) => {
            eprintln!("[job {}] could not run: {error}", job.id)
        }
    }
}
