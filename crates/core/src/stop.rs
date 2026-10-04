//! What a signal stops. SIGINT, SIGTERM or SIGHUP (an agent harness giving
//! up, a closed terminal) would otherwise end the process and leave its work
//! running elsewhere: a child in a process group of its own, which the
//! terminal's SIGINT never reaches, or a statement on a busy database, which
//! carries on after its client is gone. So whatever outlives the process
//! registers how to stop it ([`on_stop`]), and the first signal runs every
//! such stop at once, waits for them at most [`GRACE`], says on stderr what
//! it stopped and exits 128 + the signal. A second signal exits at once.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

type Stop = Box<dyn FnOnce() + Send>;

/// How long the stops get before the process exits anyway.
const GRACE: Duration = Duration::from_secs(2);

static STOPS: Mutex<BTreeMap<u64, (String, Stop)>> = Mutex::new(BTreeMap::new());
static NEXT: AtomicU64 = AtomicU64::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);

fn stops() -> MutexGuard<'static, BTreeMap<u64, (String, Stop)>> {
    STOPS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Work a signal must stop, registered until this is dropped (the work
/// ended by itself).
#[must_use = "dropping it unregisters the stop"]
pub struct OnStop(u64);

impl Drop for OnStop {
    fn drop(&mut self) {
        stops().remove(&self.0);
    }
}

/// Registers `stop`, named `what` on the signal's stderr line (`kubectl (pid
/// 42)`, `the session on reporting`). It runs on a thread of its own, so
/// it may block; past [`GRACE`] it is abandoned. Registered after a signal,
/// it runs at once: the process is on its way out.
pub fn on_stop(what: impl Into<String>, stop: impl FnOnce() + Send + 'static) -> OnStop {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut stops = stops();
    if STOPPING.load(Ordering::SeqCst) {
        drop(stops);
        thread::spawn(stop);
    } else {
        stops.insert(id, (what.into(), Box::new(stop)));
    }
    OnStop(id)
}

/// True once a signal is being handled: what the work says as its stop
/// lands (ORA-01013, a lost connection) is not news then.
pub(crate) fn stopping() -> bool {
    STOPPING.load(Ordering::SeqCst)
}

/// Every registered stop; from now on one runs as it registers.
fn take_all() -> BTreeMap<u64, (String, Stop)> {
    let mut stops = stops();
    STOPPING.store(true, Ordering::SeqCst);
    std::mem::take(&mut *stops)
}

/// Runs `taken` at once and waits for them, at most `grace`. Their names,
/// and the names of those still running at the end.
fn run_all(taken: BTreeMap<u64, (String, Stop)>, grace: Duration) -> (Vec<String>, Vec<String>) {
    let (done, finished) = mpsc::channel();
    let mut names = Vec::new();
    for (index, (_, (what, stop))) in taken.into_iter().enumerate() {
        names.push(what);
        let done = done.clone();
        thread::spawn(move || {
            stop();
            let _ = done.send(index);
        });
    }
    drop(done);
    let mut pending: Vec<bool> = vec![true; names.len()];
    let until = Instant::now() + grace;
    while let Ok(index) = finished.recv_timeout(until.saturating_duration_since(Instant::now())) {
        pending[index] = false;
    }
    let slow = names
        .iter()
        .zip(&pending)
        .filter(|(_, pending)| **pending)
        .map(|(name, _)| name.clone())
        .collect();
    (names, slow)
}

/// The one line a signal leaves on stderr.
fn said(signal: &str, stopped: &[String], slow: &[String]) -> String {
    let mut line = format!("agent-cli: {signal}: ");
    if stopped.is_empty() {
        line.push_str("nothing was running");
    } else {
        line.push_str(&format!("stopped {}", stopped.join(", ")));
    }
    if !slow.is_empty() {
        line.push_str(&format!(
            "; {} still stopping after {} s",
            slow.join(", "),
            GRACE.as_secs()
        ));
    }
    line
}

/// Handles SIGINT, SIGTERM and SIGHUP for the rest of the process. Only
/// [`crate::run`] calls it: an in-process test keeps the default handling.
#[cfg(unix)]
pub(crate) fn handle_signals() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    // A signal the parent ignored stays ignored: `nohup` keeps SIGHUP from
    // ending the run, and a shell's background job is not the terminal's
    // to interrupt.
    let ignored = inherited_ignores();
    let wanted = [SIGINT, SIGTERM, SIGHUP]
        .into_iter()
        .filter(|signal| ignored & (1u64 << (signal - 1)) == 0);
    let Ok(mut signals) = signal_hook::iterator::Signals::new(wanted) else {
        return;
    };
    thread::spawn(move || {
        let mut signals = signals.forever();
        let Some(first) = signals.next() else {
            return;
        };
        thread::spawn(move || {
            let (stopped, slow) = run_all(take_all(), GRACE);
            let name = match first {
                SIGINT => "SIGINT",
                SIGTERM => "SIGTERM",
                _ => "SIGHUP",
            };
            eprintln!("{}", said(name, &stopped, &slow));
            std::process::exit(128 + first);
        });
        if let Some(second) = signals.next() {
            std::process::exit(128 + second);
        }
    });
}

#[cfg(not(unix))]
pub(crate) fn handle_signals() {}

/// The signals this process started out ignoring, as the kernel's mask (bit
/// n - 1 for signal n).
// ponytail: read from Linux's /proc, so elsewhere an ignored signal is
// handled anyway; sigaction through a safe crate if that matters.
#[cfg(unix)]
fn inherited_ignores() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix("SigIgn:"))
        .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_stop_leaves_the_registry_when_its_work_ends() {
        let registered = on_stop("finished work", || panic!("an ended stop never runs"));
        let id = registered.0;
        assert!(stops().contains_key(&id));
        drop(registered);
        assert!(!stops().contains_key(&id));
    }

    // The registry is the process's, shared with every test that starts a
    // child, so this one runs a map of its own.
    #[test]
    fn a_signal_runs_every_stop_at_once_and_names_the_slow_ones() {
        let (ran, heard) = mpsc::channel();
        let mut taken: BTreeMap<u64, (String, Stop)> = BTreeMap::new();
        let kill: Stop = Box::new(move || {
            let _ = ran.send("kubectl");
        });
        taken.insert(0, ("kubectl (pid 7)".into(), kill));
        let stuck: Stop = Box::new(|| thread::sleep(Duration::from_secs(5)));
        taken.insert(1, ("the statement on ora".into(), stuck));
        let started = Instant::now();
        let (stopped, slow) = run_all(taken, Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(stopped, ["kubectl (pid 7)", "the statement on ora"]);
        assert_eq!(slow, ["the statement on ora"]);
        assert_eq!(heard.recv().unwrap(), "kubectl");
        assert_eq!(
            said("SIGINT", &stopped, &slow),
            "agent-cli: SIGINT: stopped kubectl (pid 7), the statement on ora; the statement on \
             ora still stopping after 2 s"
        );
        assert_eq!(
            said("SIGTERM", &[], &[]),
            "agent-cli: SIGTERM: nothing was running"
        );
    }
}
