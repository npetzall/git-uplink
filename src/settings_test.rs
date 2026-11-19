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
