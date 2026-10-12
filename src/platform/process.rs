// Child processes with a time limit, so a hung network tool or test command cannot hang Crane.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/** What a finished child process produced
 * Fields
    - success: bool - exit status was success
    - code: Option<i32> - exit code, None when killed by a signal or the time limit
    - stdout: String - standard output (lossy UTF-8)
    - stderr: String - standard error (lossy UTF-8)
    - timed_out: bool - the time limit was reached and the process was killed
    - duration_ms: u64 - wall-clock run time
*/
#[derive(Debug, Clone)]
pub(crate) struct Output {
    pub(crate) success: bool,
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
    pub(crate) duration_ms: u64,
}

/** Windows process creation flag: run a console program with a hidden console of its own */
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/** Keep a non-interactive child (git, curl, gh, test commands) from opening a console window. On
 * Windows a console program started by a process without a console (an editor, an agent host, or
 * the background sync) otherwise gets a new visible console that flashes open and closed; its
 * output is captured or discarded, so the window is never needed. Nothing changes elsewhere.
 * Input
    - command: &mut Command - command to configure
 * Output
    - &mut Command, for chaining
*/
pub(crate) fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/** Run a command with captured output and a time limit; output is read on helper threads so a
 * chatty child cannot block on a full pipe
 * Input
    - command: &mut Command - configured command (stdin is set to null)
    - timeout: Duration - time limit
 * Output
    - Result<Output, String>
    - Error if the command cannot be started
*/
pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    let started = Instant::now();
    let mut child = hidden(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start {:?}: {error}", command.get_program()))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(stream) = stdout.as_mut() {
            let _ = stream.read_to_end(&mut text);
        }
        text
    });
    let err = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(stream) = stderr.as_mut() {
            let _ = stream.read_to_end(&mut text);
        }
        text
    });
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break Some(status);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    Ok(Output {
        success: status.is_some_and(|status| status.success()),
        code: status.and_then(|status| status.code()),
        stdout,
        stderr,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/** Build a command that runs a shell command line: cmd /C on Windows, sh -c elsewhere
 * Input
    - line: &str - shell command line
 * Output
    - Command
*/
pub(crate) fn shell(line: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", line]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", line]);
        command
    }
}
