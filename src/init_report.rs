use crate::types::AssessCheck;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitReport {
    pub ok: bool,
    pub checks: Vec<AssessCheck>,
}

impl InitReport {
    pub fn from_checks(checks: Vec<AssessCheck>) -> Self {
        let ok = !checks.iter().any(|c| c.status == "fail");
        Self { ok, checks }
    }

    pub fn empty() -> Self {
        Self {
            ok: true,
            checks: Vec::new(),
        }
    }

    pub fn failed_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status == "fail").count()
    }
}

pub fn format_init_summary(report: &InitReport) -> String {
    if report.ok {
        "Uplink init: ready".into()
    } else {
        format!("Uplink init: incomplete ({} failed)", report.failed_count())
    }
}
