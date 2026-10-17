//! CLI settings in `uplink.toml` on the company-only branch `uplink/hooks`.
//!
//! `init` asks for them and writes the file. Every command reads the file
//! straight from the branch, so a developer's machine and CI agree.

use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::git::{GitOpts, git};
use crate::types::Forge;

pub const SETTINGS_PATH: &str = "uplink.toml";

const HEADER: &str = "# Uplink settings. The CLI reads this file from branch uplink/hooks.\n";
const SOURCES: [&str; 2] = [
    "refs/heads/uplink/hooks:uplink.toml",
    "refs/remotes/origin/uplink/hooks:uplink.toml",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKey {
    BannedWords,
}

impl SettingKey {
    pub const ALL: [SettingKey; 1] = [SettingKey::BannedWords];

    pub fn name(self) -> &'static str {
        match self {
            Self::BannedWords => "banned_words",
        }
    }

    fn comment(self) -> &'static str {
        match self {
            Self::BannedWords => {
                "Words that must not appear in a contribution, such as company names and internal domains (case-insensitive, matched anywhere)."
            }
        }
    }

    /// The question `init` asks for this setting.
    pub fn question(self) -> &'static str {
        match self {
            Self::BannedWords => {
                "Words that must not appear in a contribution, such as company names and internal domains (comma-separated, empty for none)"
            }
        }
    }
}

/// Keys an older binary wrote, replaced by `banned_words`. A file that still
/// has one fails the scan, so its entries are never silently dropped.
const RETIRED_KEYS: [&str; 2] = ["redact_keywords", "internal_email_domains"];

/// The settings the CLI works with. `problem` is set when `uplink.toml`
/// should exist but is missing or does not parse; see [`Settings::usable`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub banned_words: Vec<String>,
    pub problem: Option<String>,
}

impl Settings {
    /// Refuses settings that cannot be trusted, so a scan never passes
    /// because there was nothing to look for.
    pub fn usable(&self) -> Result<&Self> {
        match &self.problem {
            Some(problem) => Err(Error::msg(problem.clone())),
            None => Ok(self),
        }
    }

    fn value(&self, key: SettingKey) -> toml::Value {
        let list = |items: &[String]| {
            toml::Value::Array(items.iter().cloned().map(toml::Value::String).collect())
        };
        match key {
            SettingKey::BannedWords => list(&self.banned_words),
        }
    }

    fn entry(&self, key: SettingKey) -> String {
        format!(
            "\n# {}\n{} = {}\n",
            key.comment(),
            key.name(),
            self.value(key)
        )
    }

    /// The whole file, with a comment above each setting.
    pub fn render(&self) -> String {
        let mut text = HEADER.to_string();
        for key in SettingKey::ALL {
            text.push_str(&self.entry(key));
        }
        text
    }

    /// `text` with `keys` appended from `self`. Everything already in the
    /// file is kept byte for byte.
    pub fn append_to(&self, text: &str, keys: &[SettingKey]) -> Result<String> {
        if text.lines().any(|line| line.trim_start().starts_with('[')) {
            return Err(Error::msg(format!(
                "{SETTINGS_PATH} on uplink/hooks has a table header, so settings cannot be \
appended safely. Add {} above the first table by hand.",
                keys.iter()
                    .map(|key| key.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        for key in keys {
            out.push_str(&self.entry(*key));
        }
        Ok(out)
    }
}

/// Answers given on the command line (`init --banned-word …`).
#[derive(Debug, Clone, Default)]
pub struct SettingsFlags {
    /// Not a setting: the command `init` seeds `preflight.sh` with.
    pub preflight: Option<String>,
    pub banned_words: Option<Vec<String>>,
}

/// Answers `keys` in order of preference: the flag, a question in a terminal
/// (offering `legacy`, the value an older `queue.json` held), else `legacy`.
/// Also returns the settings that ended up empty without having been asked.
pub fn answer_settings(
    keys: &[SettingKey],
    flags: &SettingsFlags,
    legacy: &Settings,
    interactive: bool,
) -> Result<(Settings, Vec<&'static str>)> {
    let mut answers = Settings::default();
    let mut unanswered = Vec::new();
    for key in keys {
        let flag = match key {
            SettingKey::BannedWords => flags.banned_words.clone(),
        };
        let default = match key {
            SettingKey::BannedWords => legacy.banned_words.clone(),
        };
        let asked = flag.is_some() || interactive;
        let items = match flag {
            Some(items) => items,
            None if interactive => {
                vec![crate::prompt::ask_line(
                    key.question(),
                    &default.join(", "),
                )?]
            }
            None => default,
        };
        match key {
            SettingKey::BannedWords => answers.banned_words = clean_list(items),
        }
        let empty = match key {
            SettingKey::BannedWords => answers.banned_words.is_empty(),
        };
        if empty && !asked {
            unanswered.push(key.name());
        }
    }
    Ok((answers, unanswered))
}

/// `uplink.toml` as written: every key is optional so a file from an older
/// binary still parses, and the missing keys can be asked for.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsFile {
    banned_words: Option<Vec<String>>,
    redact_keywords: Option<toml::Value>,
    internal_email_domains: Option<toml::Value>,
}

impl SettingsFile {
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|err| {
            Error::msg(format!(
                "{SETTINGS_PATH} on uplink/hooks does not parse: {}",
                err.to_string().trim_end()
            ))
        })
    }

    pub fn missing_keys(&self) -> Vec<SettingKey> {
        let mut missing = Vec::new();
        if self.banned_words.is_none() {
            missing.push(SettingKey::BannedWords);
        }
        missing
    }

    /// Why the file cannot be used even though it parses: it still has keys
    /// an older binary wrote.
    pub fn problem(&self) -> Option<String> {
        let present = [
            self.redact_keywords.is_some(),
            self.internal_email_domains.is_some(),
        ];
        let retired: Vec<&str> = RETIRED_KEYS
            .into_iter()
            .zip(present)
            .filter_map(|(key, present)| present.then_some(key))
            .collect();
        (!retired.is_empty()).then(|| {
            format!(
                "{SETTINGS_PATH} on uplink/hooks still has {}; move the entries into \
banned_words and delete the old keys",
                retired.join(", ")
            )
        })
    }

    pub fn into_settings(self) -> Settings {
        let problem = self.problem();
        Settings {
            banned_words: clean_list(self.banned_words.unwrap_or_default()),
            problem,
        }
    }
}

/// Trims, drops empties, and splits comma-separated items, so a flag given
/// once with commas and a flag repeated mean the same.
pub fn clean_list(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        for part in item.split(',') {
            let part = part.trim();
            if !part.is_empty() && !out.iter().any(|seen| seen == part) {
                out.push(part.to_string());
            }
        }
    }
    out
}

/// `uplink.toml` from the local `uplink/hooks`, else from origin's.
pub fn read_settings_text(repo: &Path) -> Option<String> {
    SOURCES
        .iter()
        .find_map(|source| settings_text_at(repo, source))
}

/// `uplink.toml` at `source` (`<rev>:uplink.toml`), if that blob exists.
pub fn settings_text_at(repo: &Path, source: &str) -> Option<String> {
    let result = git(repo, &["cat-file", "blob", source], GitOpts::allow_fail()).ok()?;
    (result.code == 0).then_some(result.stdout)
}

/// Never fails: a queue must stay readable (for `init`, `status`, `doctor`)
/// when the file is absent or broken. Commands that depend on the settings
/// call [`Settings::usable`].
pub fn load_settings(repo: &Path, forge: Option<Forge>) -> Settings {
    let Some(text) = read_settings_text(repo) else {
        return Settings {
            // Without a forge there is no uplink/hooks to hold the file.
            problem: forge.map(|_| {
                format!(
                    "{SETTINGS_PATH} is missing on uplink/hooks; run git uplink init --upgrade \
to create it, then git uplink push"
                )
            }),
            ..Settings::default()
        };
    };
    match SettingsFile::parse(&text) {
        Ok(file) => file.into_settings(),
        Err(err) => Settings {
            problem: Some(err.to_string()),
            ..Settings::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        Settings {
            banned_words: vec!["acme".into(), "say \"hi\"".into()],
            problem: None,
        }
    }

    #[test]
    fn rendered_file_parses_back_to_the_same_settings() {
        let text = sample().render();
        assert!(text.starts_with(HEADER), "{text}");
        assert!(text.contains("banned_words = [\"acme\", "), "{text}");
        let file = SettingsFile::parse(&text).unwrap();
        assert!(file.missing_keys().is_empty());
        assert_eq!(file.into_settings(), sample());
    }

    #[test]
    fn empty_answers_render_as_empty_values() {
        let text = Settings::default().render();
        assert!(text.contains("banned_words = []"), "{text}");
        let file = SettingsFile::parse(&text).unwrap();
        assert!(file.missing_keys().is_empty());
        assert_eq!(file.into_settings(), Settings::default());
    }

    #[test]
    fn an_empty_file_reports_its_missing_keys() {
        let file = SettingsFile::parse("").unwrap();
        assert_eq!(file.missing_keys(), [SettingKey::BannedWords]);
        assert_eq!(file.into_settings(), Settings::default());
    }

    #[test]
    fn a_key_that_is_no_longer_a_setting_is_ignored() {
        let file = SettingsFile::parse("preflight = \"npm test\"\nbanned_words = []\n").unwrap();
        assert!(file.missing_keys().is_empty());
        assert_eq!(file.into_settings(), Settings::default());
    }

    #[test]
    fn a_retired_key_makes_the_settings_unusable() {
        let text = "redact_keywords = [\"acme\"]\ninternal_email_domains = []\nbanned_words = []\n";
        let settings = SettingsFile::parse(text).unwrap().into_settings();
        let err = settings.usable().unwrap_err().to_string();
        assert!(
            err.contains("redact_keywords, internal_email_domains"),
            "{err}"
        );
        assert!(err.contains("banned_words"), "{err}");
    }

    #[test]
    fn appending_keeps_the_existing_text() {
        let existing = "# ours\npreflight = \"npm test\"";
        let text = sample()
            .append_to(existing, &[SettingKey::BannedWords])
            .unwrap();
        assert!(
            text.starts_with("# ours\npreflight = \"npm test\"\n"),
            "{text}"
        );
        let settings = SettingsFile::parse(&text).unwrap().into_settings();
        assert_eq!(settings, sample());
    }

    #[test]
    fn appending_refuses_a_file_with_tables() {
        let err = sample()
            .append_to("[extra]\nkey = 1\n", &[SettingKey::BannedWords])
            .unwrap_err()
            .to_string();
        assert!(err.contains("table header"), "{err}");
        assert!(err.contains("banned_words"), "{err}");
    }

    #[test]
    fn a_broken_file_is_an_error_that_names_it() {
        let err = SettingsFile::parse("banned_words = [\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains(SETTINGS_PATH), "{err}");
    }

    #[test]
    fn lists_split_on_commas_and_drop_duplicates() {
        assert_eq!(
            clean_list(vec!["a, b".into(), "b".into(), " ".into(), "c".into()]),
            ["a", "b", "c"]
        );
    }
}
