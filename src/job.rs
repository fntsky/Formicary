use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// A shared handle to a scheduled shell job.
///
/// Cloning a `Job` keeps a reference to the same job, so changes made through
/// `JobService` are visible to every holder of the handle.
#[derive(Clone)]
pub struct Job {
    pub(crate) inner: Arc<RwLock<JobSnapshot>>,
}

/// A read-only copy of a job's current configuration and state.
#[derive(Clone, Debug)]
pub struct JobSnapshot {
    pub id: u64,
    pub name: String,
    pub cron_expression: String,
    pub working_directory: PathBuf,
    pub shell_command: String,
    pub timeout: Duration,
    pub enabled: bool,
}

impl Job {
    pub(crate) fn new(snapshot: JobSnapshot) -> Self {
        Self {
            inner: Arc::new(RwLock::new(snapshot)),
        }
    }

    pub fn id(&self) -> u64 {
        self.inner.read().expect("job lock poisoned").id
    }

    pub fn snapshot(&self) -> JobSnapshot {
        self.inner.read().expect("job lock poisoned").clone()
    }

    pub(crate) fn update(
        &self,
        name: String,
        cron_expression: String,
        working_directory: PathBuf,
        shell_command: String,
        timeout: Duration,
    ) {
        let mut snapshot = self.inner.write().expect("job lock poisoned");
        snapshot.name = name;
        snapshot.cron_expression = cron_expression;
        snapshot.working_directory = working_directory;
        snapshot.shell_command = shell_command;
        snapshot.timeout = timeout;
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.inner.write().expect("job lock poisoned").enabled = enabled;
    }
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Job").field(&self.snapshot()).finish()
    }
}
