use serde::{Deserialize, Deserializer, Serialize};

pub const QUEUE_PATH: &str = ".uplink/queue.json";
pub const PATCH_DIR: &str = ".uplink/patches";
pub const STATE_BRANCH: &str = "uplink/state";
pub const DEFAULT_CUTOFF: &str = "----- Uplink: internal below this line -----";
pub const TOOLING_PATCH_KIND: &str = "uplink-tooling";
pub const TOOLING_PATCH_TITLE: &str = "Uplink tooling";
pub const QUEUE_VERSION: u32 = 2;

fn default_state_branch() -> String {
    STATE_BRANCH.to_string()
}

pub const DEFAULT_EXPORT_AUTHOR: (&str, &str) =
    ("Uplink Contributor", "uplink@users.noreply.github.com");

pub type PatchStatus = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchLayer {
    Tooling,
    Upstream,
    Internal,
}

impl PatchLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tooling => "tooling",
            Self::Upstream => "upstream",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Forge {
    Ghec,
    ExampleGithub,
}

impl Forge {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ghec => "ghec",
            Self::ExampleGithub => "example-github",
        }
    }

    pub fn family(self) -> ForgeFamily {
        ForgeFamily::Github
    }
}

impl std::fmt::Display for Forge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeFamily {
    Github,
}

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
pub struct PatchApproval {
    pub at: String,
    pub version: u32,
    pub kind: String,
    pub sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_id_stable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "issueNumber"
    )]
    pub issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "issueUrl")]
    pub issue_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_message: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<PatchApproval>,
    pub events: Vec<PatchEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl Patch {
    pub fn last_approval(&self) -> Option<&PatchApproval> {
        self.approvals.last()
    }
}

fn skip_empty_option(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|s| s.is_empty())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConfig {
    pub upstream_remote: String,
    pub upstream_branch: String,
    pub contrib_remote: String,
    #[serde(alias = "companyBranch")]
    pub internal_branch: String,
    #[serde(default, skip_serializing_if = "skip_empty_option")]
    pub upstream_url: Option<String>,
    #[serde(default, skip_serializing_if = "skip_empty_option")]
    pub contrib_url: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<Forge>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            upstream_remote: "upstream".into(),
            upstream_branch: "main".into(),
            contrib_remote: "contrib".into(),
            internal_branch: "main".into(),
            upstream_url: None,
            contrib_url: None,
            state_branch: STATE_BRANCH.into(),
            trailer_key: "Uplink-Patch-Id".into(),
            preflight_command: None,
            cutoff_marker: Some(DEFAULT_CUTOFF.into()),
            export_author_name: Some(DEFAULT_EXPORT_AUTHOR.0.into()),
            export_author_email: Some(DEFAULT_EXPORT_AUTHOR.1.into()),
            redact_keywords: Vec::new(),
            internal_email_domains: Vec::new(),
            forge: None,
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
#[serde(rename_all = "camelCase")]
pub struct PendingUpstream {
    pub sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_sha: Option<String>,
    pub at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flowed_back: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_commits: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueState {
    pub version: u32,
    pub config: QueueConfig,
    #[serde(rename = "lastSync", skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<LastSync>,
    #[serde(rename = "pendingUpstream", skip_serializing_if = "Option::is_none")]
    pub pending_upstream: Option<PendingUpstream>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooling: Option<Patch>,
    #[serde(default)]
    pub upstream: Vec<Patch>,
    #[serde(default)]
    pub internal: Vec<Patch>,
}

impl QueueState {
    pub fn empty(config: QueueConfig) -> Self {
        Self {
            version: QUEUE_VERSION,
            config,
            last_sync: None,
            pending_upstream: None,
            tooling: None,
            upstream: Vec::new(),
            internal: Vec::new(),
        }
    }

    pub fn all_patches(&self) -> impl Iterator<Item = &Patch> {
        self.tooling
            .iter()
            .chain(self.upstream.iter())
            .chain(self.internal.iter())
    }

    pub fn all_patches_mut(&mut self) -> impl Iterator<Item = &mut Patch> {
        self.tooling
            .iter_mut()
            .chain(self.upstream.iter_mut())
            .chain(self.internal.iter_mut())
    }

    pub fn patch_refs(&self) -> Vec<&Patch> {
        self.all_patches().collect()
    }

    pub fn layer_of(&self, id: &str) -> Option<PatchLayer> {
        if self.tooling.as_ref().is_some_and(|p| p.id == id) {
            Some(PatchLayer::Tooling)
        } else if self.upstream.iter().any(|p| p.id == id) {
            Some(PatchLayer::Upstream)
        } else if self.internal.iter().any(|p| p.id == id) {
            Some(PatchLayer::Internal)
        } else {
            None
        }
    }

    pub fn is_tooling(&self, id: &str) -> bool {
        matches!(self.layer_of(id), Some(PatchLayer::Tooling))
    }

    pub fn is_upstream(&self, id: &str) -> bool {
        matches!(self.layer_of(id), Some(PatchLayer::Upstream))
    }

    pub fn is_internal(&self, id: &str) -> bool {
        matches!(self.layer_of(id), Some(PatchLayer::Internal))
    }

    pub fn push_patch(&mut self, patch: Patch, internal: bool) {
        if internal {
            self.internal.push(patch);
        } else {
            self.upstream.push(patch);
        }
    }
}

impl<'de> Deserialize<'de> for QueueState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = QueueStateWire::deserialize(deserializer)?;
        Ok(wire.into_queue())
    }
}

#[derive(Deserialize)]
struct QueueStateWire {
    #[allow(dead_code)]
    version: u32,
    config: QueueConfig,
    #[serde(rename = "lastSync", default)]
    last_sync: Option<LastSync>,
    #[serde(rename = "pendingUpstream", default)]
    pending_upstream: Option<PendingUpstream>,
    #[serde(default)]
    patches: Vec<PatchWire>,
    #[serde(default)]
    tooling: Option<PatchWire>,
    #[serde(default)]
    upstream: Vec<PatchWire>,
    #[serde(default)]
    internal: Vec<PatchWire>,
}

impl QueueStateWire {
    fn into_queue(self) -> QueueState {
        let layered =
            self.tooling.is_some() || !self.upstream.is_empty() || !self.internal.is_empty();
        let (tooling, upstream, internal) = if layered {
            (
                self.tooling.map(Patch::from),
                self.upstream.into_iter().map(Patch::from).collect(),
                self.internal.into_iter().map(Patch::from).collect(),
            )
        } else {
            migrate_v1_patches(self.patches)
        };
        QueueState {
            version: QUEUE_VERSION,
            config: self.config,
            last_sync: self.last_sync,
            pending_upstream: self.pending_upstream,
            tooling,
            upstream,
            internal,
        }
    }
}

fn migrate_v1_patches(patches: Vec<PatchWire>) -> (Option<Patch>, Vec<Patch>, Vec<Patch>) {
    let mut tooling = None;
    let mut upstream = Vec::new();
    let mut internal = Vec::new();
    for raw in patches {
        let is_tooling = raw.kind.as_deref() == Some(TOOLING_PATCH_KIND);
        let is_internal = raw.intent.as_deref() == Some("internal-only");
        let patch = Patch::from(raw);
        if is_tooling && tooling.is_none() {
            tooling = Some(patch);
        } else if is_internal {
            internal.push(patch);
        } else {
            upstream.push(patch);
        }
    }
    (tooling, upstream, internal)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchWire {
    id: String,
    title: String,
    #[serde(default)]
    commit_message: String,
    #[serde(default)]
    intent: Option<String>,
    status: String,
    #[serde(default)]
    depends_on: Vec<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    patch_id_stable: Option<String>,
    #[serde(default)]
    source: PatchSource,
    #[serde(default)]
    prepare: Option<PrepareReport>,
    #[serde(default)]
    upstream: Option<PatchUpstream>,
    #[serde(default)]
    merged: Option<PatchMerged>,
    #[serde(default)]
    conflict: Option<PatchConflict>,
    #[serde(default)]
    approvals: Vec<PatchApproval>,
    #[serde(default)]
    events: Vec<PatchEvent>,
    #[serde(default)]
    kind: Option<String>,
}

impl From<PatchWire> for Patch {
    fn from(raw: PatchWire) -> Self {
        Self {
            id: raw.id,
            title: raw.title,
            commit_message: raw.commit_message,
            status: raw.status,
            depends_on: raw.depends_on,
            created_at: raw.created_at,
            updated_at: raw.updated_at,
            patch_id_stable: raw.patch_id_stable,
            source: raw.source,
            prepare: raw.prepare,
            upstream: raw.upstream,
            merged: raw.merged,
            conflict: raw.conflict,
            approvals: raw.approvals,
            events: raw.events,
            kind: raw.kind,
        }
    }
}
