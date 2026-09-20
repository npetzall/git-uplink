use std::fmt;

use crate::types::AssessReport;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Git(#[from] crate::git::GitError),
    #[error("{0}")]
    Preflight(#[from] PreflightError),
    #[error("{0}")]
    Assess(#[from] AssessError),
    #[error("{0}")]
    Conflict(#[from] ConflictError),
}

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

#[derive(Debug)]
pub struct PreflightError {
    message: String,
    pub suggested_depends_on: Vec<String>,
    pub stage: &'static str,
    pub output: Option<String>,
}

impl PreflightError {
    pub fn new(
        message: impl Into<String>,
        suggested_depends_on: Vec<String>,
        stage: &'static str,
        output: Option<String>,
    ) -> Self {
        Self {
            message: message.into(),
            suggested_depends_on,
            stage,
            output,
        }
    }
}

impl fmt::Display for PreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PreflightError {}

#[derive(Debug)]
pub struct AssessError {
    message: String,
    pub report: Box<AssessReport>,
}

impl AssessError {
    pub fn new(message: impl Into<String>, report: AssessReport) -> Self {
        Self {
            message: message.into(),
            report: Box::new(report),
        }
    }
}

impl fmt::Display for AssessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AssessError {}

#[derive(Debug)]
pub struct ConflictError {
    message: String,
    pub patch_id: String,
    pub files: Vec<String>,
}

impl ConflictError {
    pub fn new(
        message: impl Into<String>,
        patch_id: impl Into<String>,
        files: Vec<String>,
    ) -> Self {
        Self {
            message: message.into(),
            patch_id: patch_id.into(),
            files,
        }
    }
}

impl fmt::Display for ConflictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConflictError {}
