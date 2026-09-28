use std::io::{IsTerminal, Write};

use indicatif::{ProgressBar, ProgressStyle};

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
    spinner: Option<ProgressBar>,
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

    fn record(&mut self, id: &str, outcome: StepOutcome) {
        self.checks.push(AssessCheck {
            id: id.into(),
            status: outcome.status,
            detail: outcome.detail,
        });
    }

    fn start_spinner(&mut self, label: &str) {
        if self.tty {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
            );
            pb.enable_steady_tick(std::time::Duration::from_millis(80));
            pb.set_message(label.to_string());
            self.spinner = Some(pb);
        }
    }

    fn finish_line(&mut self, label: &str, outcome: &StepOutcome) {
        if let Some(pb) = self.spinner.take() {
            pb.finish_and_clear();
        }
        let line = format_step_line(self.tty, label, outcome);
        let _ = writeln!(std::io::stderr(), "{line}");
    }
}

pub fn format_step_line(tty: bool, label: &str, outcome: &StepOutcome) -> String {
    if tty {
        let (icon, color) = match outcome.status {
            CheckStatus::Pass => ("✓", GREEN),
            CheckStatus::Fail => ("✗", RED),
            CheckStatus::Warn => ("!", YELLOW),
            CheckStatus::Skip => ("−", DIM),
        };
        let detail = if outcome.detail.is_empty() {
            String::new()
        } else {
            format!("  {}{}", DIM, outcome.detail)
        };
        format!("{color}{icon}{RESET} {label}{detail}{RESET}")
    } else {
        let tag = match outcome.status {
            CheckStatus::Pass => "ok  ",
            CheckStatus::Fail => "fail",
            CheckStatus::Warn => "warn",
            CheckStatus::Skip => "skip",
        };
        if outcome.detail.is_empty() {
            format!("[{tag}] {label}")
        } else {
            format!("[{tag}] {label} — {detail}", detail = outcome.detail)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_step_line_tty_icons() {
        let pass = StepOutcome::pass("done");
        assert!(format_step_line(true, "Fetch upstream", &pass).contains("✓"));
        let fail = StepOutcome::fail("no token");
        assert!(format_step_line(true, "Fetch upstream", &fail).contains("✗"));
    }

    #[test]
    fn format_step_line_plain_tags() {
        let pass = StepOutcome::pass("done");
        let line = format_step_line(false, "Fetch upstream", &pass);
        assert!(line.starts_with("[ok  ]"));
    }
}
