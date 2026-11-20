use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::types::{AssessCheck, CheckStatus};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProgressMode {
    #[default]
    Disabled,
    Auto,
    Plain,
}

#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub status: CheckStatus,
    pub detail: String,
}

impl StepOutcome {
    pub fn pass(detail: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Pass,
            detail: detail.into(),
        }
    }

    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Fail,
            detail: detail.into(),
        }
    }

    pub fn skip(detail: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Skip,
            detail: detail.into(),
        }
    }

    pub fn warn(detail: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Warn,
            detail: detail.into(),
        }
    }
}

pub struct StepProgress {
    mode: ProgressMode,
    tty: bool,
    checks: Vec<AssessCheck>,
    spinner: Option<Spinner>,
}

impl StepProgress {
    pub fn from_mode(mode: ProgressMode) -> Self {
        let tty = mode == ProgressMode::Auto && std::io::stderr().is_terminal();
        Self {
            mode,
            tty,
            checks: Vec::new(),
            spinner: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.mode != ProgressMode::Disabled
    }

    pub fn checks(&self) -> &[AssessCheck] {
        &self.checks
    }

    pub fn run_check(&mut self, id: &str, label: &str, f: impl FnOnce() -> StepOutcome) {
        if !self.enabled() {
            let outcome = f();
            self.record(id, outcome);
            return;
        }
        self.start_spinner(label);
        let outcome = f();
        self.finish_line(label, &outcome);
        self.record(id, outcome);
    }

    pub fn run_step<T>(
        &mut self,
        id: &str,
        label: &str,
        f: impl FnOnce() -> crate::error::Result<(T, StepOutcome)>,
    ) -> crate::error::Result<T> {
        if !self.enabled() {
            match f() {
                Ok((value, outcome)) => {
                    self.record(id, outcome);
                    Ok(value)
                }
                Err(err) => {
                    self.record(id, StepOutcome::fail(err.to_string()));
                    Err(err)
                }
            }
        } else {
            self.start_spinner(label);
            match f() {
                Ok((value, outcome)) => {
                    self.finish_line(label, &outcome);
                    self.record(id, outcome);
                    Ok(value)
                }
                Err(err) => {
                    let outcome = StepOutcome::fail(err.to_string());
                    self.finish_line(label, &outcome);
                    self.record(id, outcome);
                    Err(err)
                }
            }
        }
    }

    /// Starts the spinner of a line that [`Self::finish`] ends, for a step
    /// whose outcome comes in parts.
    pub fn start(&mut self, label: &str) {
        if self.enabled() {
            self.start_spinner(label);
        }
    }

    /// Ends the spinner [`Self::start`] began with `outcome`, or prints
    /// another line for `label` when none runs.
    pub fn finish(&mut self, label: &str, outcome: &StepOutcome) {
        if self.enabled() {
            self.finish_line(label, outcome);
        }
    }

    fn record(&mut self, id: &str, outcome: StepOutcome) {
        self.checks.push(AssessCheck {
            id: id.into(),
            status: outcome.status,
            detail: outcome.detail,
        });
    }

    fn start_spinner(&mut self, label: &str) {
        if self.tty {
            self.spinner = Some(Spinner::start(label.to_string()));
        }
    }

    fn finish_line(&mut self, label: &str, outcome: &StepOutcome) {
        if let Some(spinner) = self.spinner.take() {
            spinner.finish_and_clear();
        }
        let line = format_step_line(self.tty, label, outcome);
        let _ = writeln!(std::io::stderr(), "{line}");
    }
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A spinner and `label` redrawn on one stderr line until
/// [`Self::finish_and_clear`] or a drop stops it and blanks the line.
struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    fn start(label: String) -> Self {
        let label = printable(&label);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            for frame in SPINNER_FRAMES.iter().cycle() {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let mut err = std::io::stderr().lock();
                let _ = write!(err, "\r{frame} {label}");
                let _ = err.flush();
                drop(err);
                std::thread::park_timeout(Duration::from_millis(80));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn finish_and_clear(self) {
        drop(self);
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        self.stop.store(true, Ordering::Relaxed);
        handle.thread().unpark();
        let _ = handle.join();
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    }
}

/// `text` with every control character but `\t` replaced by `?`, so a label
/// or detail cannot carry escape sequences to the terminal.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() && c != '\t' { '?' } else { c })
        .collect()
}

pub fn format_step_line(tty: bool, label: &str, outcome: &StepOutcome) -> String {
    let label = printable(label);
    let detail = printable(&outcome.detail);
    if tty {
        let (icon, color) = match outcome.status {
            CheckStatus::Pass => ("✓", GREEN),
            CheckStatus::Fail => ("✗", RED),
            CheckStatus::Warn => ("!", YELLOW),
            CheckStatus::Skip => ("−", DIM),
        };
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!("  {DIM}{detail}")
        };
        format!("{color}{icon}{RESET} {label}{detail}{RESET}")
    } else {
        let tag = match outcome.status {
            CheckStatus::Pass => "ok  ",
            CheckStatus::Fail => "fail",
            CheckStatus::Warn => "warn",
            CheckStatus::Skip => "skip",
        };
        if detail.is_empty() {
            format!("[{tag}] {label}")
        } else {
            format!("[{tag}] {label} — {detail}")
        }
    }
}

#[cfg(test)]
#[path = "progress_test.rs"]
mod tests;
