use serde::{Deserialize, Serialize};

pub const QUEUE_PATH: &str = ".uplink/queue.json";
pub const PATCH_DIR: &str = ".uplink/patches";
pub const STATE_BRANCH: &str = "uplink/state";
pub const DEFAULT_CUTOFF: &str = "----- Uplink: internal below this line -----";

fn default_state_branch() -> String {
    STATE_BRANCH.to_string()
}

pub const DEFAULT_EXPORT_AUTHOR: (&str, &str) =
    ("Uplink Contributor", "uplink@users.noreply.github.com");

pub type PatchIntent = String;
pub type PatchStatus = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MergeVia {
    Pr,
    Trailer,
    #[serde(rename = "patch-id")]
    PatchId,
    #[serde(rename = "empty-rebase")]
    EmptyRebase,
    Manual,
}

impl MergeVia {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Trailer => "trailer",
            Self::PatchId => "patch-id",
            Self::EmptyRebase => "empty-rebase",
            Self::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pr" => Some(Self::Pr),
            "trailer" => Some(Self::Trailer),
            "patch-id" => Some(Self::PatchId),
            "empty-rebase" => Some(Self::EmptyRebase),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchEvent {
    pub at: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareCheck {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareReport {
    pub at: String,
    pub ok: bool,
    #[serde(default)]
    pub commit_message: String,
    pub public_subject: String,
    pub public_body: String,
    pub author_name: String,
    pub author_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_email: Option<String>,
    pub cutoff_found: bool,
    pub checks: Vec<PrepareCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_pr_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_pr_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchUpstream {
    pub contrib_branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchMerged {
    pub via: MergeVia,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchConflict {
    pub branch: String,
    pub files: Vec<String>,
    pub message: String,
    /// Prefix commit the conflict branch was cut from (upstream + earlier patches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onto: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_message: String,
    pub intent: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub patch_id_stable: Option<String>,
    pub source: PatchSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepare: Option<PrepareReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<PatchUpstream>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged: Option<PatchMerged>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<PatchConflict>,
    pub events: Vec<PatchEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConfig {
    pub upstream_remote: String,
    pub upstream_branch: String,
    pub contrib_remote: String,
    pub company_branch: String,
    #[serde(default = "default_state_branch")]
    pub state_branch: String,
    pub trailer_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preflight_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff_marker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_author_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_author_email: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact_keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_email_domains: Vec<String>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            upstream_remote: "upstream".into(),
            upstream_branch: "main".into(),
            contrib_remote: "contrib".into(),
            company_branch: "main".into(),
            state_branch: STATE_BRANCH.into(),
            trailer_key: "Uplink-Patch-Id".into(),
            preflight_command: None,
            cutoff_marker: Some(DEFAULT_CUTOFF.into()),
            export_author_name: Some(DEFAULT_EXPORT_AUTHOR.0.into()),
            export_author_email: Some(DEFAULT_EXPORT_AUTHOR.1.into()),
            redact_keywords: Vec::new(),
            internal_email_domains: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastSync {
    pub at: String,
    #[serde(rename = "upstreamSha")]
    pub upstream_sha: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueState {
    pub version: u32,
    pub config: QueueConfig,
    #[serde(rename = "lastSync", skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<LastSync>,
    pub patches: Vec<Patch>,
}

impl QueueState {
    pub fn empty(config: QueueConfig) -> Self {
        Self {
            version: 1,
            config,
            last_sync: None,
            patches: Vec::new(),
        }
    }
}
