use std::io;
use std::path::Path;
use std::process::{Command, Output};

/// Runs shell command text in the supplied working directory.
///
/// The command is interpreted by the platform shell, so callers must not pass
/// command text built from untrusted input. A non-zero exit code is returned in
/// `Output::status`; `io::Error` is reserved for failures to start the process.
pub struct ShellRunner;

impl ShellRunner {
    pub fn run(working_directory: impl AsRef<Path>, command_text: &str) -> io::Result<Output> {
        #[cfg(windows)]
        let mut process = {
            let mut process = Command::new("cmd.exe");
            process.args(["/C", command_text]);
            process
        };

        #[cfg(not(windows))]
        let mut process = {
            let mut process = Command::new("sh");
            process.args(["-c", command_text]);
            process
        };

        process.current_dir(working_directory).output()
    }
}
