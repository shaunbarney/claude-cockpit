//! Run an external command with a hard deadline.
//!
//! Every collector shells out (`docker`, `lsof`, `ss`, `git`). A wedged Docker
//! daemon or a hung NFS mount would otherwise freeze a refresh thread forever,
//! so these calls go through [`output_with_timeout`]: the child is killed once
//! the deadline passes and the caller sees `None`, exactly as if the tool were
//! missing.
use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Default deadline for quick CLI probes (`docker ps`, `lsof`, `ss`, `git`).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Run `cmd`, capturing stdout/stderr, and kill it if it outlives `timeout`.
///
/// Returns `None` if the program can't be spawned or timed out. A non-zero
/// exit status is *not* an error here — `lsof` exits 1 when nothing matches —
/// so callers inspect `Output::status` themselves when they care.
pub fn output_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    // Drain both pipes on their own threads: a chatty child would otherwise
    // block on a full pipe buffer and look exactly like a hang.
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let status = wait_deadline(&mut child, timeout)?;
    Some(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Convenience: run `program args…` with `timeout` and return stdout as a
/// (lossy UTF-8) string; empty on spawn failure or timeout.
pub fn stdout_of(program: &str, args: &[&str], timeout: Duration) -> String {
    output_with_timeout(Command::new(program).args(args), timeout)
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    })
}

/// Poll the child until it exits or the deadline passes (then kill + reap).
fn wait_deadline(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    let mut nap = Duration::from_millis(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => {
                thread::sleep(nap);
                nap = (nap * 2).min(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout() {
        let s = stdout_of("sh", &["-c", "echo hi"], Duration::from_secs(5));
        assert_eq!(s.trim(), "hi");
    }

    #[test]
    fn kills_on_timeout() {
        let start = Instant::now();
        let out = output_with_timeout(
            Command::new("sh").args(["-c", "sleep 5"]),
            Duration::from_millis(100),
        );
        assert!(out.is_none());
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn missing_program_is_none() {
        assert!(output_with_timeout(
            &mut Command::new("definitely-not-a-real-binary-xyz"),
            DEFAULT_TIMEOUT
        )
        .is_none());
    }
}
