use super::{
    AuthRole, GitOpts, build_command, git_http_extra_header, http_auth_config_args, http_origin,
    is_local_transport, named_auth_role, network_remote_index, ssh_to_https, transport_from_role,
    urls_match,
};

#[test]
fn offline_refuses_network_commands_and_restores_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("origin.git");
    let repo = dir.path().join("work");
    std::fs::create_dir_all(&repo).unwrap();
    super::git_ok(
        dir.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    )
    .unwrap();
    super::git_ok(&repo, &["init", "-q"]).unwrap();
    super::git_ok(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]).unwrap();
    super::git_ok(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
    super::with_offline(|| {
        for args in [
            &["fetch", "origin"][..],
            &["push", "origin", "HEAD:refs/heads/main"],
            &["ls-remote", "origin"],
            &["remote", "update"],
        ] {
            let err = super::git(&repo, args, GitOpts::allow_fail()).unwrap_err();
            assert!(err.to_string().contains("needs the network"), "{err}");
        }
        super::git_ok(&repo, &["rev-parse", "HEAD"]).unwrap();
    });
    assert!(!super::is_offline());
    let pushed = super::git_ok(&bare, &["for-each-ref", "refs/heads"]).unwrap();
    assert!(pushed.is_empty(), "{pushed}");
}

#[test]
fn git_succeeds_reports_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    super::git_ok(repo, &["init", "-q"]).unwrap();
    let has_head = |repo| super::git_succeeds(repo, &["rev-parse", "--verify", "-q", "HEAD"]);
    assert!(!has_head(repo).unwrap());
    super::git_ok(repo, &["commit", "-q", "--allow-empty", "-m", "init"]).unwrap();
    assert!(has_head(repo).unwrap());
}

#[test]
fn git_http_header_is_basic_x_access_token() {
    let header = git_http_extra_header("test-token");
    assert_eq!(
        header,
        "Authorization: Basic eC1hY2Nlc3MtdG9rZW46dGVzdC10b2tlbg=="
    );
    assert!(header.ends_with("=="), "standard base64 must keep padding");
    assert!(!header.contains("Bearer"));
    assert!(!header.contains('\n'));

    let padded = git_http_extra_header("a");
    assert_eq!(padded, "Authorization: Basic eC1hY2Nlc3MtdG9rZW46YQ==");
    assert!(!padded.contains('\n'));
}

#[test]
fn token_header_is_passed_in_env_not_argv() {
    let opts = GitOpts {
        extra_env: vec![("UPLINK_INTERNAL_TOKEN".into(), "pat-token".into())],
        ..GitOpts::default()
    };
    let transport =
        transport_from_role("https://github.com/acme/app.git", AuthRole::Internal, &opts).unwrap();
    let cmd = build_command(
        std::path::Path::new("."),
        &["fetch", "origin"],
        &opts,
        &transport,
    );
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !args
            .iter()
            .any(|a| a.contains("Authorization") || a.contains("extraheader")),
        "{args:?}"
    );
    let envs: Vec<(String, String)> = cmd
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                v?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    let get = |key: &str| envs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    assert_eq!(get("GIT_CONFIG_COUNT"), Some("3"));
    assert_eq!(get("GIT_CONFIG_KEY_0"), Some("credential.helper"));
    assert_eq!(get("GIT_CONFIG_VALUE_0"), Some(""));
    assert_eq!(
        get("GIT_CONFIG_KEY_2"),
        Some("http.https://github.com/.extraheader")
    );
    assert!(
        get("GIT_CONFIG_VALUE_2").is_some_and(|v| v.starts_with("Authorization: Basic ")),
        "{envs:?}"
    );
}

#[test]
fn network_remote_index_skips_flags() {
    assert_eq!(
        network_remote_index(&[
            "fetch",
            "--quiet",
            "--prune",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main"
        ]),
        Some(3)
    );
    assert_eq!(
        network_remote_index(&[
            "push",
            "--force-with-lease=refs/heads/main:abc",
            "origin",
            "HEAD:refs/heads/main",
        ]),
        Some(2)
    );
    assert_eq!(
        network_remote_index(&["clone", "--quiet", "/tmp/repo.git", "/tmp/dest"]),
        Some(2)
    );
    assert_eq!(
        network_remote_index(&[
            "fetch",
            "--quiet",
            "--end-of-options",
            "--upload-pack=x",
            "+refs/heads/main:refs/remotes/x/main",
        ]),
        Some(3)
    );
    assert_eq!(network_remote_index(&["commit", "-m", "msg"]), None);
    assert_eq!(network_remote_index(&["remote", "get-url", "origin"]), None);
}

#[test]
fn ssh_urls_rewrite_to_https() {
    assert_eq!(
        ssh_to_https("git@github.com:acme/app.git").as_deref(),
        Some("https://github.com/acme/app.git")
    );
    assert_eq!(
        ssh_to_https("ssh://git@github.example.com/acme/app.git").as_deref(),
        Some("https://github.example.com/acme/app.git")
    );
    assert_eq!(
        ssh_to_https("ssh://git@github.com:22/acme/app.git").as_deref(),
        Some("https://github.com/acme/app.git")
    );
    assert_eq!(ssh_to_https("https://github.com/acme/app.git"), None);
}

#[test]
fn http_origin_matches_checkout_extraheader_subsection() {
    assert_eq!(
        http_origin("https://github.com/acme/app.git").as_deref(),
        Some("https://github.com")
    );
    assert_eq!(
        http_origin("https://github.example.com/acme/app.git").as_deref(),
        Some("https://github.example.com")
    );
    assert_eq!(
        http_origin("http://127.0.0.1:12345/repo.git").as_deref(),
        Some("http://127.0.0.1:12345")
    );
    assert_eq!(
        http_origin("https://github.com:443/acme/app.git").as_deref(),
        Some("https://github.com")
    );
    assert_eq!(
        http_origin("http://example.com:80/x").as_deref(),
        Some("http://example.com")
    );
    assert_eq!(
        http_origin("https://github.com:8443/acme/app.git").as_deref(),
        Some("https://github.com:8443")
    );
    assert_eq!(
        http_origin("https://x-access-token:tok@github.com/acme/app.git").as_deref(),
        Some("https://github.com")
    );
    assert_eq!(
        http_origin(&ssh_to_https("git@github.com:acme/app.git").unwrap()).as_deref(),
        Some("https://github.com")
    );
    assert_eq!(
        http_origin(&ssh_to_https("ssh://git@github.example.com/acme/app.git").unwrap()).as_deref(),
        Some("https://github.example.com")
    );
    assert_eq!(http_origin("file:///tmp/bare.git"), None);
    assert_eq!(http_origin("git@github.com:acme/app.git"), None);
}

#[test]
fn file_and_path_remotes_are_local() {
    assert!(is_local_transport("/tmp/bare.git"));
    assert!(is_local_transport("file:///tmp/bare.git"));
    assert!(is_local_transport("../bare.git"));
    assert!(!is_local_transport("git@github.com:acme/app.git"));
    assert!(!is_local_transport(
        "ssh://git@example.invalid/org/repo.git"
    ));
    assert!(!is_local_transport("https://github.com/acme/app.git"));
}

#[test]
fn named_remotes_map_to_auth_roles() {
    assert_eq!(named_auth_role("origin"), Some(AuthRole::Internal));
    assert_eq!(named_auth_role("contrib"), Some(AuthRole::Contrib));
    assert_eq!(named_auth_role("upstream"), Some(AuthRole::Upstream));
    assert_eq!(named_auth_role("other"), None);
}

#[test]
fn ssh_and_https_remote_urls_match() {
    assert!(urls_match(
        "git@github.com:acme/app.git",
        "https://github.com/acme/app"
    ));
    assert!(!urls_match(
        "https://github.com/acme/app.git",
        "https://github.com/acme/other.git"
    ));
}

#[test]
fn key_wins_over_token_for_role() {
    let opts = GitOpts {
        extra_env: vec![
            ("UPLINK_INTERNAL_KEY".into(), "/tmp/id_uplink".into()),
            ("UPLINK_INTERNAL_TOKEN".into(), "pat-token".into()),
        ],
        ..GitOpts::default()
    };
    let transport =
        transport_from_role("git@github.com:acme/app.git", AuthRole::Internal, &opts).unwrap();
    assert!(
        transport
            .ssh_command
            .as_deref()
            .is_some_and(|cmd| cmd.contains("/tmp/id_uplink") && cmd.contains("BatchMode=yes")),
        "key should drive GIT_SSH_COMMAND: {:?}",
        transport.ssh_command
    );
    assert!(transport.extra_header.is_none());
    assert!(transport.remote_url.is_none());
}

#[test]
fn token_rewrites_ssh_origin_to_https() {
    let opts = GitOpts {
        extra_env: vec![("UPLINK_INTERNAL_TOKEN".into(), "pat-token".into())],
        ..GitOpts::default()
    };
    let transport =
        transport_from_role("git@github.com:acme/app.git", AuthRole::Internal, &opts).unwrap();
    assert_eq!(
        transport.remote_url.as_deref(),
        Some("https://github.com/acme/app.git")
    );
    assert!(
        transport
            .extra_header
            .as_deref()
            .is_some_and(|h| h.starts_with("Authorization: Basic "))
    );
    assert!(transport.ssh_command.is_none());
}

#[test]
fn missing_role_creds_name_that_role() {
    let opts = GitOpts::default();
    let err = transport_from_role(
        "ssh://git@github.com/acme/app.git",
        AuthRole::Upstream,
        &opts,
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("UPLINK_UPSTREAM_KEY"));
    assert!(message.contains("UPLINK_UPSTREAM_TOKEN"));
    assert!(!message.contains("UPLINK_CONTRIB_TOKEN"));
}

#[test]
fn https_upstream_without_creds_is_anonymous() {
    let transport = transport_from_role(
        "HTTPS://GitHub.com/acme/app.git",
        AuthRole::Upstream,
        &GitOpts::default(),
    )
    .unwrap();
    assert!(transport.extra_header.is_none());
    assert!(transport.ssh_command.is_none());
    assert!(transport.remote_url.is_none());
    assert!(transport.isolate_gitconfig);
    assert!(transport.clear_http_auth);
    assert_eq!(transport.http_origin.as_deref(), Some("https://GitHub.com"));
    let args = http_auth_config_args(&transport);
    assert!(args.iter().any(|arg| arg == "credential.helper="));
    assert!(
        args.iter()
            .any(|arg| arg == "http.https://GitHub.com/.extraheader=")
    );
    assert!(!args.iter().any(|arg| arg.contains("Authorization")));
}

#[test]
fn https_upstream_token_is_used() {
    let opts = GitOpts {
        extra_env: vec![("UPLINK_UPSTREAM_TOKEN".into(), "pat-token".into())],
        ..GitOpts::default()
    };
    let transport =
        transport_from_role("https://github.com/acme/app.git", AuthRole::Upstream, &opts).unwrap();
    assert!(
        transport
            .extra_header
            .as_deref()
            .is_some_and(|header| header.starts_with("Authorization: Basic "))
    );
    assert!(transport.remote_url.is_some());
    assert!(!transport.clear_http_auth);
    let args = http_auth_config_args(&transport);
    assert!(args.iter().any(|arg| arg.contains("Authorization: Basic ")));
}

#[test]
fn http_upstream_without_creds_fails_closed() {
    let err = transport_from_role(
        "http://github.com/acme/app.git",
        AuthRole::Upstream,
        &GitOpts::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("UPLINK_UPSTREAM_TOKEN"));
}

#[test]
fn https_origin_without_creds_fails_closed() {
    let err = transport_from_role(
        "https://github.com/acme/app.git",
        AuthRole::Internal,
        &GitOpts::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("UPLINK_INTERNAL_TOKEN"));
}
