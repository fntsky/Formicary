use std::error::Error;
use std::fmt;
use std::io;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum ShellRunnerError {
    Io(io::Error),
    TimedOut { timeout: Duration },
}

impl fmt::Display for ShellRunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "shell command failed: {error}"),
            Self::TimedOut { timeout } => {
                write!(f, "shell command exceeded its {timeout:?} timeout")
            }
        }
    }
}

impl Error for ShellRunnerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::TimedOut { .. } => None,
        }
    }
}

/// Runs shell command text in the supplied working directory.
///
/// The command is interpreted by the platform shell, so callers must not pass
/// command text built from untrusted input. A non-zero exit code is returned in
/// `Output::status`; `io::Error` is reserved for failures to start the process.
pub struct ShellRunner;

impl ShellRunner {
    pub fn run(working_directory: impl AsRef<Path>, command_text: &str) -> io::Result<Output> {
        command_for(command_text)
            .current_dir(working_directory)
            .output()
    }

    /// Runs a shell command and terminates it if it exceeds `timeout`.
    pub fn run_with_timeout(
        working_directory: impl AsRef<Path>,
        command_text: &str,
        timeout: Duration,
    ) -> Result<Output, ShellRunnerError> {
        let mut command = command_for(command_text);
        configure_process_group(&mut command);
        let mut child = command
            .current_dir(working_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(ShellRunnerError::Io)?;

        let stdout = child.stdout.take().expect("stdout was configured as piped");
        let stderr = child.stderr.take().expect("stderr was configured as piped");
        let stdout_reader = read_pipe(stdout);
        let stderr_reader = read_pipe(stderr);
        let started_at = Instant::now();

        let mut status = None;
        let timed_out = loop {
            if status.is_none() {
                match child.try_wait() {
                    Ok(exit_status) => status = exit_status,
                    Err(error) => {
                        stop_process_tree(&mut child);
                        let _ = child.wait();
                        let _ = join_pipe(stdout_reader);
                        let _ = join_pipe(stderr_reader);
                        return Err(ShellRunnerError::Io(error));
                    }
                }
            }

            if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
                break false;
            }

            if started_at.elapsed() >= timeout {
                stop_process_tree(&mut child);
                break true;
            }

            let remaining = timeout.saturating_sub(started_at.elapsed());
            thread::sleep(remaining.min(Duration::from_millis(10)));
        };

        let status = match status {
            Some(status) => status,
            None => child.wait().map_err(ShellRunnerError::Io)?,
        };
        let stdout = join_pipe(stdout_reader).map_err(ShellRunnerError::Io)?;
        let stderr = join_pipe(stderr_reader).map_err(ShellRunnerError::Io)?;
        if timed_out {
            return Err(ShellRunnerError::TimedOut { timeout });
        }

        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

fn command_for(command_text: &str) -> Command {
    #[cfg(windows)]
    let command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", command_text]);
        command
    };

    #[cfg(not(windows))]
    let command = {
        let mut command = Command::new("sh");
        command.args(["-c", command_text]);
        command
    };

    command
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

fn read_pipe<R: Read + Send + 'static>(mut reader: R) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn join_pipe(reader: JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| io::Error::other("shell output reader thread panicked"))?
}

fn stop_process_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }

    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .status();
    }

    let _ = child.kill();
}
