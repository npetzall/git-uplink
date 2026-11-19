use super::*;

#[test]
fn config_defaults_and_overrides() {
    let cfg = SelfConfig::parse("").unwrap();
    assert_eq!(cfg.repo, DEFAULT_REPO);
    assert_eq!(cfg.signer, default_signer(DEFAULT_REPO));

    let cfg = SelfConfig::parse("repo = \"acme/git-uplink\"\n").unwrap();
    assert_eq!(
        cfg.signer,
        "https://github.com/acme/git-uplink/.github/workflows/release.yml@refs/heads/main"
    );

    let cfg =
        SelfConfig::parse("repo = \"acme/x\"\nsigner = \"https://example.com/id\"\n").unwrap();
    assert_eq!(cfg.signer, "https://example.com/id");
}

#[test]
fn config_rejects_bad_values() {
    assert!(SelfConfig::parse("repo = \"acme\"").is_err());
    assert!(SelfConfig::parse("repo = \"acme/../x\"").is_err());
    assert!(SelfConfig::parse("repo = \"a/b?c\"").is_err());
    assert!(SelfConfig::parse("signer = \"http://x\"").is_err());
    assert!(SelfConfig::parse("unknown = 1").is_err());
}

#[test]
fn versions_normalize() {
    assert_eq!(normalize_version("v1.2.3").unwrap(), "1.2.3");
    assert_eq!(normalize_version("1.2.3").unwrap(), "1.2.3");
    for bad in ["1.2", "v1.2.3-rc1", "latest", "1..3", "a.b.c", ""] {
        assert!(normalize_version(bad).is_err(), "{bad}");
    }
}

#[test]
fn sums_lookup() {
    let sums =
        "AAAA  git-uplink-x86_64-unknown-linux-musl\nbbbb *git-uplink-aarch64-apple-darwin\n";
    assert_eq!(
        expected_sha256(sums, "git-uplink-x86_64-unknown-linux-musl").as_deref(),
        Some("aaaa")
    );
    assert_eq!(
        expected_sha256(sums, "git-uplink-aarch64-apple-darwin").as_deref(),
        Some("bbbb")
    );
    assert_eq!(expected_sha256(sums, "git-uplink-aarch64"), None);
}

#[test]
fn version_output_match() {
    assert!(reports_version("git-uplink 0.5.0 (abc1234)\n", "0.5.0"));
    assert!(reports_version("git-uplink 0.5.0\n", "0.5.0"));
    assert!(!reports_version("git-uplink 0.5.01 (abc)", "0.5.0"));
    assert!(!reports_version("git-uplink 0.4.0 (abc)", "0.5.0"));
    assert!(!reports_version("other 0.5.0", "0.5.0"));
}

#[cfg(unix)]
#[test]
fn switch_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let env = SelfEnv {
        home: dir.path().to_path_buf(),
        releases_base: String::new(),
        api_base: String::new(),
    };
    fs::create_dir_all(env.bin()).unwrap();
    for v in ["0.9.0", "0.10.0", "0.2.1"] {
        fs::write(env.bin().join(format!("{PREFIX}{v}")), "").unwrap();
    }
    fs::write(env.bin().join("git-uplink-junk"), "").unwrap();
    assert_eq!(installed(&env).unwrap(), ["0.10.0", "0.9.0", "0.2.1"]);
    assert_eq!(active(&env), None);
    switch_link(&env, "0.9.0").unwrap();
    assert_eq!(active(&env).as_deref(), Some("0.9.0"));
    switch_link(&env, "0.10.0").unwrap();
    assert_eq!(
        fs::read_link(env.bin().join(LINK)).unwrap(),
        PathBuf::from("git-uplink-0.10.0")
    );
}
