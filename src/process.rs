use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    pub installed: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

pub trait CliProbe {
    fn probe(&self, name: &str) -> ProbeResult;
}

pub struct RealCliProbe {
    cache: Mutex<HashMap<String, ProbeResult>>,
}

impl RealCliProbe {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn probe_uncached(&self, name: &str) -> ProbeResult {
        let located = locate(name);
        let path = match located {
            Ok(Some(path)) => path,
            Ok(None) => return ProbeResult::default(),
            Err(error) => {
                return ProbeResult {
                    error: Some(error.to_string()),
                    ..ProbeResult::default()
                };
            }
        };
        let mut command = Command::new(&path);
        command.arg("--version");
        match run(&mut command, PROBE_TIMEOUT) {
            Ok((status, stdout, stderr)) => {
                let text = if stdout.trim().is_empty() {
                    stderr.trim()
                } else {
                    stdout.trim()
                };
                ProbeResult {
                    installed: true,
                    version: (!text.is_empty()).then(|| first_line(text)),
                    error: (!status.success()).then(|| format!("version command exited {status}")),
                }
            }
            Err(error) => ProbeResult {
                installed: true,
                error: Some(error.to_string()),
                ..ProbeResult::default()
            },
        }
    }
}

impl CliProbe for RealCliProbe {
    fn probe(&self, name: &str) -> ProbeResult {
        if let Some(result) = self.cache.lock().unwrap().get(name).cloned() {
            return result;
        }
        let result = self.probe_uncached(name);
        self.cache
            .lock()
            .unwrap()
            .insert(name.to_string(), result.clone());
        result
    }
}

pub fn shared() -> &'static RealCliProbe {
    static PROBE: OnceLock<RealCliProbe> = OnceLock::new();
    PROBE.get_or_init(RealCliProbe::new)
}

impl RealCliProbe {
    /// Cache-only lookup so UI code never blocks on a subprocess.
    pub fn probe_cached(&self, name: &str) -> Option<ProbeResult> {
        self.cache.lock().unwrap().get(name).cloned()
    }
}

/// Kick off CLI detection on a background thread; `probe_cached` picks the
/// results up once they land instead of freezing first-frame rendering.
pub fn warm_up(names: &[&'static str]) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let names: Vec<&'static str> = names.to_vec();
    std::thread::spawn(move || {
        let probe = shared();
        for name in names {
            probe.probe(name);
        }
    });
}

fn locate(name: &str) -> Result<Option<String>> {
    let mut command = Command::new(if cfg!(windows) { "where" } else { "which" });
    command.arg(name);
    let (status, stdout, _) = run(&mut command, PROBE_TIMEOUT)?;
    Ok(status
        .success()
        .then(|| stdout.lines().find(|line| !line.trim().is_empty()))
        .flatten()
        .map(|line| line.trim().to_string()))
}

fn run(
    command: &mut Command,
    timeout: Duration,
) -> Result<(std::process::ExitStatus, String, String)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(command);
    let mut child = command.spawn().context("start CLI probe")?;
    // Drain both pipes concurrently: waiting for exit first would deadlock any
    // child that fills the OS pipe buffer before exiting.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_handle = std::thread::spawn(move || read_output(stdout_pipe));
    let stderr_handle = std::thread::spawn(move || read_output(stderr_pipe));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            anyhow::bail!("CLI probe timed out after {} seconds", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let collect = |handle: std::thread::JoinHandle<anyhow::Result<String>>| -> String {
        handle
            .join()
            .unwrap_or_else(|_| Ok(String::new()))
            .unwrap_or_default()
    };
    Ok((status, collect(stdout_handle), collect(stderr_handle)))
}

fn read_output<R: Read>(reader: Option<R>) -> Result<String> {
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        reader.take(MAX_OUTPUT_BYTES).read_to_end(&mut bytes)?;
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn hide_window(_: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeProbe {
        calls: Cell<usize>,
        result: ProbeResult,
    }

    impl CliProbe for FakeProbe {
        fn probe(&self, _: &str) -> ProbeResult {
            self.calls.set(self.calls.get() + 1);
            self.result.clone()
        }
    }

    #[test]
    fn probe_interface_reports_installed_version() {
        let probe = FakeProbe {
            calls: Cell::new(0),
            result: ProbeResult {
                installed: true,
                version: Some("agy 2.1".into()),
                error: None,
            },
        };
        let result = probe.probe("agy");
        assert!(result.installed);
        assert_eq!(result.version.as_deref(), Some("agy 2.1"));
        assert_eq!(probe.calls.get(), 1);
    }
}
