//! Child processes (`az`, `kubectl`, `kubelogin`), bounded by the deadline.
//!
//! Ported from az-tui's `run_until`. stdin is null, so nothing can wait on a
//! prompt; both pipes are drained on their own threads, so a child that fills
//! one never blocks; and the child leads its own process group, so the kill at
//! the deadline also takes down a credential plugin it started, such as a
//! kubelogin sitting on a device-code prompt. The terminal's SIGINT does not
//! reach that group, so a signal to agent-cli kills it too ([`crate::on_stop`]).

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::ctx::{Ctx, Op};
use crate::error::Failure;

/// How a finished child exited, and both of its pipes whole.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `command` to completion, or kills its process group at `deadline`.
///
/// A program that is not there is exit 3 ("not installed or not on PATH"), and
/// one still running at the deadline is exit 124.
pub fn run_until(mut command: Command, deadline: Instant) -> Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| -> anyhow::Error {
            if error.kind() == std::io::ErrorKind::NotFound {
                Failure::setup(format!("{program} is not installed or not on PATH"))
                    .hint(format!("install {program}, or put it on PATH"))
                    .into()
            } else {
                anyhow::Error::new(error).context(format!("{program} could not be started"))
            }
        })?;
    let leader = child.id();
    let _stop = crate::stop::on_stop(format!("{program} (pid {leader})"), move || {
        kill_group(leader);
    });
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("{program} could not be waited for"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            // The drains are not joined: a grandchild that left the group
            // still holds the pipes open for as long as it runs, which is the
            // very wait the deadline is for.
            // ponytail: the group kill misses a grandchild that started its own
            // session, and Windows has no group here; a job object if that shows.
            return Err(Failure::timed_out(format!(
                "{program} was still running at the deadline and was stopped"
            ))
            .hint(format!(
                "run it again with a larger --timeout, or run {program} by hand to see what it waits for"
            ))
            .into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    // What the child left running in its group (`cmd &`) holds the pipes
    // open for as long as it runs; the child is done, so that goes too.
    kill_group(child.id());
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Kills the process group the child leads (it was started as its leader).
fn kill_group(leader: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{leader}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(not(unix))]
    let _ = leader;
}

/// Reads one pipe to its end on a thread of its own.
fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// A child process is an op: `ctx.read(Command::new("kubectl")...)` for a
/// look, `ctx.write(effect, ...)` for a change. It cannot tell which it is, so
/// the handler's choice of door is the declaration.
impl Op for Command {
    type Output = Output;

    fn plan(&self) -> Value {
        let argv: Vec<String> = std::iter::once(self.get_program())
            .chain(self.get_args())
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        json!({ "run": argv })
    }

    fn perform(self, ctx: &Ctx) -> Result<Output> {
        run_until(self, ctx.deadline())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Exit, describe};

    #[test]
    fn a_finished_child_hands_back_its_status_and_both_pipes() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo out; echo err >&2; exit 3"]);
        let output = run_until(command, Instant::now() + Duration::from_secs(10)).unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(
            (output.stdout.as_str(), output.stderr.as_str()),
            ("out\n", "err\n")
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_child_that_leaves_a_process_behind_returns_when_it_does() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf secret; (sleep 15 &)"]);
        let started = Instant::now();
        let output = run_until(command, Instant::now() + Duration::from_secs(10)).unwrap();
        assert_eq!(output.stdout, "secret");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_missing_program_is_needs_setup() {
        let error = run_until(
            Command::new("agent-cli-no-such-program"),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap_err();
        let (exit, message, _) = describe(&error);
        assert_eq!(exit, Exit::Setup);
        assert_eq!(
            message,
            "agent-cli-no-such-program is not installed or not on PATH"
        );
    }

    #[test]
    fn the_deadline_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let mut command = Command::new("sh");
        // The grandchild writes its pid and outlives the shell unless the
        // group is killed.
        command.args([
            "-c",
            &format!("sleep 30 & echo $! > {}; wait", pid_file.display()),
        ]);
        let started = Instant::now();
        let error = run_until(command, Instant::now() + Duration::from_millis(300)).unwrap_err();
        assert_eq!(describe(&error).0, Exit::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .to_owned();
        let gone = (0..100).any(|_| {
            let alive = Command::new("kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            // A killed child can linger as a zombie until it is reaped.
            let zombie = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid])
                .output()
                .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim().starts_with('Z'));
            if alive && !zombie {
                thread::sleep(Duration::from_millis(20));
            }
            !alive || zombie
        });
        assert!(gone, "the grandchild {pid} survived the deadline");
    }

    #[test]
    fn a_plan_is_the_argv() {
        let mut command = Command::new("kubectl");
        command.args(["delete", "pod", "api-1"]);
        assert_eq!(
            command.plan(),
            json!({"run": ["kubectl", "delete", "pod", "api-1"]})
        );
    }
}
