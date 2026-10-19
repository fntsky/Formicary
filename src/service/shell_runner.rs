use std::error::Error;
use std::fmt;
use std::io;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
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

/// Shared cancellation parameter; use a fresh control for each execution.
#[derive(Clone, Debug, Default)]
pub struct ShellControl {
    cancelled: Arc<AtomicBool>,
}

impl ShellControl {
    pub fn terminate(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_terminated(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// A snapshot of bytes captured so far, including output without a newline.
#[derive(Clone, Debug, Default)]
pub struct ShellLogs {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Cloneable input handle for sending bytes or commands to a running shell.
#[derive(Clone)]
pub struct ShellInput {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

impl ShellInput {
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| io::Error::other("shell input lock poisoned"))?;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "shell input is closed"))?;
        stdin.write_all(bytes)?;
        stdin.flush()
    }

    /// Sends a complete command line. The input is interpreted by the running program.
    pub fn send_line(&self, line: &str) -> io::Result<()> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        self.write(&bytes)
    }

    /// Sends EOF; affects all clones of this input handle.
    pub fn close(&self) {
        self.stdin.lock().expect("shell input lock poisoned").take();
    }
}

/// Running command with live logs and a shared termination control.
/// Dropping the handle terminates and reaps the process tree.
pub struct ShellExecution {
    logs: Arc<Mutex<ShellLogs>>,
    input: ShellInput,
    control: ShellControl,
    worker: Option<JoinHandle<Result<Output, ShellRunnerError>>>,
}

impl ShellExecution {
    pub fn input(&self) -> ShellInput {
        self.input.clone()
    }
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.input.write(bytes)
    }
    pub fn send_line(&self, line: &str) -> io::Result<()> {
        self.input.send_line(line)
    }
    pub fn close_input(&self) {
        self.input.close();
    }

    pub fn logs(&self) -> ShellLogs {
        self.logs.lock().expect("shell logs lock poisoned").clone()
    }
    pub fn terminate(&self) {
        self.control.terminate();
    }
    pub fn is_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(|worker| worker.is_finished())
    }
    pub fn wait(mut self) -> Result<Output, ShellRunnerError> {
        self.finish()
    }

    /// Collects the result while retaining this handle and its captured logs.
    /// Call only once per execution.
    pub fn finish(&mut self) -> Result<Output, ShellRunnerError> {
        self.worker
            .take()
            .ok_or_else(|| {
                ShellRunnerError::Io(io::Error::other("shell result already collected"))
            })?
            .join()
            .map_err(|_| ShellRunnerError::Io(io::Error::other("shell worker thread panicked")))?
    }
}

impl Drop for ShellExecution {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.terminate();
            let _ = worker.join();
            self.input.close();
        }
    }
}

impl ShellRunner {
    /// Starts a command, capturing live logs. Cancellation returns its exit Output.
    /// Log buffers retain all output until the execution handle is dropped.
    pub fn start(
        working_directory: impl AsRef<Path>,
        command_text: &str,
        timeout: Option<Duration>,
        control: ShellControl,
    ) -> io::Result<ShellExecution> {
        let mut command = command_for(command_text);
        configure_process_group(&mut command);
        let mut child = command
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let input = ShellInput {
            stdin: Arc::new(Mutex::new(child.stdin.take())),
        };
        let logs = Arc::new(Mutex::new(ShellLogs::default()));
        let stdout_reader = read_pipe(child.stdout.take().unwrap(), logs.clone(), false);
        let stderr_reader = read_pipe(child.stderr.take().unwrap(), logs.clone(), true);
        let worker_control = control.clone();
        let worker = thread::spawn(move || {
            let started_at = Instant::now();
            let mut status = None;
            let mut timed_out = false;
            loop {
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
                    break;
                }
                timed_out = timeout.is_some_and(|limit| started_at.elapsed() >= limit);
                if timed_out || worker_control.is_terminated() {
                    stop_process_tree(&mut child);
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let status = match status {
                Some(status) => status,
                None => child.wait().map_err(ShellRunnerError::Io)?,
            };
            let stdout = join_pipe(stdout_reader).map_err(ShellRunnerError::Io)?;
            let stderr = join_pipe(stderr_reader).map_err(ShellRunnerError::Io)?;
            if timed_out {
                return Err(ShellRunnerError::TimedOut {
                    timeout: timeout.unwrap(),
                });
            }
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        });
        Ok(ShellExecution {
            logs,
            input,
            control,
            worker: Some(worker),
        })
    }

    /// Synchronous convenience API: stdin is closed immediately.
    pub fn run(working_directory: impl AsRef<Path>, command_text: &str) -> io::Result<Output> {
        let execution = Self::start(
            working_directory,
            command_text,
            None,
            ShellControl::default(),
        )?;
        execution.close_input();
        execution.wait().map_err(|error| match error {
            ShellRunnerError::Io(error) => error,
            error => io::Error::other(error),
        })
    }

    pub fn run_with_timeout(
        working_directory: impl AsRef<Path>,
        command_text: &str,
        timeout: Duration,
    ) -> Result<Output, ShellRunnerError> {
        let execution = Self::start(
            working_directory,
            command_text,
            Some(timeout),
            ShellControl::default(),
        )
        .map_err(ShellRunnerError::Io)?;
        execution.close_input();
        execution.wait()
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

fn read_pipe<R: Read + Send + 'static>(
    mut reader: R,
    logs: Arc<Mutex<ShellLogs>>,
    stderr: bool,
) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            output.extend_from_slice(&buffer[..count]);
            let mut logs = logs.lock().expect("shell logs lock poisoned");
            let stream = if stderr {
                &mut logs.stderr
            } else {
                &mut logs.stdout
            };
            stream.extend_from_slice(&buffer[..count]);
        }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// 验证进程退出前即可读取标准输出和错误输出，并可通过共享控制对象终止进程树。
    /// Verifies that stdout and stderr are readable before exit and a shared control terminates the running process tree.
    #[test]
    fn reads_live_logs_and_cancels_with_shared_parameter() {
        let control = ShellControl::default();
        let execution = ShellRunner::start(
            ".",
            "printf ready; printf error >&2; sleep 30",
            None,
            control.clone(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let logs = execution.logs();
            if logs.stdout == b"ready" && logs.stderr == b"error" {
                break;
            }
            assert!(Instant::now() < deadline, "live logs did not arrive");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!execution.is_finished());
        control.terminate();
        let output = execution.wait().unwrap();
        assert!(!output.status.success());
        assert_eq!(output.stdout, b"ready");
        assert_eq!(output.stderr, b"error");
        assert!(Instant::now() < deadline);
    }

    /// 验证输入句柄能向运行中的 shell 发送命令，并在 shell 退出前读取命令输出。
    /// Verifies that an input handle sends commands to a running shell and their output is available before the shell exits.
    #[test]
    fn sends_commands_to_shell_and_reads_response() {
        let execution = ShellRunner::start(
            ".",
            "exec sh",
            Some(Duration::from_secs(3)),
            ShellControl::default(),
        )
        .unwrap();
        let input = execution.input();
        input.send_line("printf 'hello from shell'").unwrap();
        input.send_line("printf 'error output' >&2").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let logs = execution.logs();
            if logs.stdout == b"hello from shell" && logs.stderr == b"error output" {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        input.send_line("exit 0").unwrap();
        assert!(execution.wait().unwrap().status.success());
    }

    /// 验证输入字节保持不变，关闭一个输入句柄会发送 EOF，并阻止其他克隆句柄继续写入。
    /// Verifies that stdin bytes are preserved and closing one handle sends EOF and prevents writes through its clones.
    #[test]
    fn closes_stdin_for_all_clones_and_preserves_bytes() {
        let execution = ShellRunner::start(
            ".",
            "cat",
            Some(Duration::from_secs(3)),
            ShellControl::default(),
        )
        .unwrap();
        let input = execution.input();
        execution.write(b"hello\nworld").unwrap();
        execution.close_input();
        assert_eq!(
            input.write(b"later").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(execution.wait().unwrap().stdout, b"hello\nworld");
    }

    /// 验证同步执行接口会关闭标准输入，使等待 EOF 的命令能够结束。
    /// Verifies that the synchronous runner closes stdin so a command waiting for EOF can finish.
    #[test]
    fn synchronous_runner_supplies_eof() {
        assert!(ShellRunner::run(".", "cat").unwrap().stdout.is_empty());
    }

    /// 验证同步执行接口保留标准输出、错误输出及非零退出码。
    /// Verifies that the synchronous runner preserves stdout, stderr, and a nonzero exit code.
    #[test]
    fn captures_output_and_exit_status() {
        let output = ShellRunner::run(".", "printf hello; printf problem >&2; exit 7").unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"hello");
        assert_eq!(output.stderr, b"problem");
    }

    /// 验证父 shell 退出后，超时机制仍能终止持有输出管道的后台子进程。
    /// Verifies that timeout terminates a background child that still holds output pipes after the parent shell exits.
    #[test]
    fn times_out_process_tree_even_after_shell_exits() {
        let started = Instant::now();
        let result = ShellRunner::run_with_timeout(".", "sleep 30 &", Duration::from_millis(50));
        assert!(matches!(result, Err(ShellRunnerError::TimedOut { .. })));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// 验证丢弃执行句柄时会及时终止并回收进程树。
    /// Verifies that dropping an execution handle terminates and reaps its process tree promptly.
    #[test]
    fn dropping_handle_terminates_process_tree() {
        let execution = ShellRunner::start(".", "sleep 30", None, ShellControl::default()).unwrap();
        let started = Instant::now();
        drop(execution);
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
