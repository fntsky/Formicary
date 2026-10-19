use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use cron::Schedule;

use crate::service::job::{Job, JobRunLog, JobRunOutcome, JobSnapshot};
use crate::service::shell_runner::{ShellControl, ShellRunner, ShellRunnerError};

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
        let job = state.job.clone();
        if !job.snapshot().enabled {
            continue;
        }
        drop(state);

        run_shell_job(&job);
    }
}

fn run_shell_job(handle: &Job) {
    let job = handle.snapshot();
    let started_at = SystemTime::now();
    let control = ShellControl::default();
    println!("[job {}] running: {}", job.id, job.shell_command);
    let execution = match ShellRunner::start(
        &job.working_directory,
        &job.shell_command,
        Some(job.timeout),
        control.clone(),
    ) {
        Ok(execution) => execution,
        Err(error) => {
            handle
                .history
                .lock()
                .expect("job history lock poisoned")
                .push(JobRunLog {
                    started_at,
                    finished_at: SystemTime::now(),
                    command: job.shell_command.clone(),
                    working_directory: job.working_directory.clone(),
                    logs: Default::default(),
                    outcome: JobRunOutcome::Failed {
                        error: error.to_string(),
                    },
                });
            eprintln!("[job {}] could not run: {error}", job.id);
            return;
        }
    };
    *handle
        .execution
        .lock()
        .expect("job execution lock poisoned") = Some(execution);
    let (result, logs) = loop {
        {
            let mut slot = handle
                .execution
                .lock()
                .expect("job execution lock poisoned");
            if slot
                .as_ref()
                .expect("running Job has a shell execution")
                .is_finished()
            {
                let mut execution = slot.take().unwrap();
                execution.close_input();
                let result = execution.finish();
                let logs = execution.logs();
                break (result, logs);
            }
        }
        thread::sleep(Duration::from_millis(10));
    };
    let outcome = match &result {
        Err(ShellRunnerError::TimedOut { .. }) => JobRunOutcome::TimedOut,
        Err(error) => JobRunOutcome::Failed {
            error: error.to_string(),
        },
        Ok(_) if control.is_terminated() => JobRunOutcome::Terminated,
        Ok(output) => JobRunOutcome::Exited {
            success: output.status.success(),
            code: output.status.code(),
        },
    };
    handle
        .history
        .lock()
        .expect("job history lock poisoned")
        .push(JobRunLog {
            started_at,
            finished_at: SystemTime::now(),
            command: job.shell_command.clone(),
            working_directory: job.working_directory.clone(),
            logs,
            outcome,
        });
    match result {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::service::shell_runner::ShellLogStream;
    fn output(
        entries: &[crate::service::shell_runner::ShellLogEntry],
        stream: crate::service::shell_runner::ShellLogStream,
    ) -> Vec<u8> {
        entries
            .iter()
            .filter(|entry| entry.stream == stream)
            .flat_map(|entry| entry.bytes.iter().copied())
            .collect()
    }

    use std::time::Instant;

    fn shell_job(command: &str, timeout: Duration) -> Job {
        Job::new(JobSnapshot {
            id: 1,
            name: "controlled".into(),
            cron_expression: "* * * * *".into(),
            working_directory: ".".into(),
            shell_command: command.into(),
            timeout,
            enabled: true,
        })
    }

    /// 验证克隆的 Job 句柄共享实时输入、日志及终止能力，并在执行后保留日志和启用状态。
    /// Verifies that cloned Job handles share live shell input, logs, and termination while preserving logs and the enabled state afterward.
    #[test]
    fn job_retains_shell_for_live_input_logs_and_termination() {
        let job = shell_job("printf ready; exec cat", Duration::from_secs(5));
        let worker_job = job.clone();
        let worker = thread::spawn(move || run_shell_job(&worker_job));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !job
            .shell_logs()
            .is_some_and(|logs| output(&logs, ShellLogStream::Stdout) == b"ready")
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(job.shell_is_running());
        job.shell_input().unwrap().send_line("hello").unwrap();
        while !job
            .shell_logs()
            .is_some_and(|logs| output(&logs, ShellLogStream::Stdout) == b"readyhello\n")
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        job.terminate_shell();
        worker.join().unwrap();
        assert!(!job.shell_is_running());
        assert!(job.shell_input().is_none());
        assert!(job.shell_logs().is_none());
        assert!(job.execution.lock().unwrap().is_none());
        let history = job.log_history();
        assert_eq!(
            output(&history[0].logs, ShellLogStream::Stdout),
            b"readyhello\n"
        );
        assert_eq!(history[0].outcome, JobRunOutcome::Terminated);
        assert!(job.snapshot().enabled);
    }

    /// 验证 shell 执行超时后，Job 仍保留已捕获的输出。
    /// Verifies that a Job retains captured output after its shell execution times out.
    #[test]
    fn job_retains_logs_after_timeout() {
        let job = shell_job(
            "printf before-timeout; sleep 30",
            Duration::from_millis(100),
        );
        run_shell_job(&job);
        assert!(!job.shell_is_running());
        assert!(job.shell_logs().is_none());
        let history = job.log_history();
        assert_eq!(
            output(&history[0].logs, ShellLogStream::Stdout),
            b"before-timeout"
        );
        assert_eq!(history[0].outcome, JobRunOutcome::TimedOut);
    }

    /// 验证多次执行的日志分别归档到对应 Job，保留退出码且不会被下一次运行覆盖。
    /// Verifies that repeated runs archive separate logs on their Job without overwriting previous output or exit codes.
    #[test]
    fn archives_each_run_on_its_own_job() {
        let job = shell_job("printf first", Duration::from_secs(3));
        run_shell_job(&job);
        job.update(
            "controlled".into(),
            "* * * * *".into(),
            ".".into(),
            "printf second; printf error >&2; exit 7".into(),
            Duration::from_secs(3),
        );
        run_shell_job(&job);
        let history = job.clone().log_history();
        assert_eq!(history.len(), 2);
        assert_eq!(output(&history[0].logs, ShellLogStream::Stdout), b"first");
        assert!(!history[0].logs.is_empty());
        assert!(
            history[0]
                .logs
                .iter()
                .all(|entry| entry.timestamp >= history[0].started_at
                    && entry.timestamp <= history[0].finished_at)
        );
        assert_eq!(
            history[0].outcome,
            JobRunOutcome::Exited {
                success: true,
                code: Some(0)
            }
        );
        assert_eq!(output(&history[1].logs, ShellLogStream::Stdout), b"second");
        assert_eq!(output(&history[1].logs, ShellLogStream::Stderr), b"error");
        assert_eq!(
            history[1].outcome,
            JobRunOutcome::Exited {
                success: false,
                code: Some(7)
            }
        );
        assert!(history.iter().all(|run| run.finished_at >= run.started_at));
        assert!(job.execution.lock().unwrap().is_none());
        assert!(
            shell_job("true", Duration::from_secs(3))
                .log_history()
                .is_empty()
        );
    }

    /// 验证启动失败也会归档错误记录，并且不会留下 shell 执行句柄。
    /// Verifies that a startup failure is archived as an error without retaining a shell execution handle.
    #[test]
    fn archives_startup_failure() {
        let job = shell_job("true", Duration::from_secs(3));
        job.inner.write().unwrap().working_directory =
            std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        run_shell_job(&job);
        assert!(matches!(
            job.log_history()[0].outcome,
            JobRunOutcome::Failed { .. }
        ));
        assert!(job.execution.lock().unwrap().is_none());
    }
}
