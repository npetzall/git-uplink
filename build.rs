use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let web = manifest.join("web");
    let dist = web.join("dist");

    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/package-lock.json");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/vite.config.ts");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=templates/github");
    println!("cargo:rerun-if-changed=templates/ghec");
    println!("cargo:rerun-if-changed=templates/example-github");
    println!("cargo:rerun-if-env-changed=GIT_UPLINK_SKIP_WEB_BUILD");

    embed_commit(&manifest);

    if env::var("GIT_UPLINK_SKIP_WEB_BUILD").ok().as_deref() == Some("1") {
        if !dist.join("index.html").is_file() {
            panic!(
                "GIT_UPLINK_SKIP_WEB_BUILD=1 but {} is missing",
                dist.join("index.html").display()
            );
        }
        return;
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    run(npm, &["ci", "--no-fund", "--no-audit"], &web);
    run(npm, &["run", "build"], &web);
    if !dist.join("index.html").is_file() {
        panic!("vite did not write web/dist/index.html");
    }
}

/// Sets `GIT_UPLINK_COMMIT` for `--version`. Packagers without a checkout can
/// pass it in; otherwise it falls back to `unknown` instead of failing the build.
fn embed_commit(manifest: &Path) {
    println!("cargo:rerun-if-env-changed=GIT_UPLINK_COMMIT");
    let commit = match env::var("GIT_UPLINK_COMMIT") {
        Ok(commit) if !commit.is_empty() => commit,
        _ => git_commit(manifest).unwrap_or_else(|| "unknown".to_string()),
    };
    println!("cargo:rustc-env=GIT_UPLINK_COMMIT={commit}");
}

fn git_commit(manifest: &Path) -> Option<String> {
    let sha = git(manifest, &["rev-parse", "--short=7", "HEAD"])?;

    // Rebuild when HEAD moves. Only watch paths that exist: cargo treats a
    // missing rerun-if-changed path as always changed.
    let mut watch = vec![git(manifest, &["rev-parse", "--git-path", "HEAD"])?];
    if let Some(head_ref) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
        watch.extend(git(manifest, &["rev-parse", "--git-path", &head_ref]));
    }
    watch.extend(git(manifest, &["rev-parse", "--git-path", "packed-refs"]));
    for path in watch {
        let path = manifest.join(path);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    // Release CI rewrites the package version in Cargo.toml and Cargo.lock
    // before building, so those two files do not count as dirty.
    let status = git(
        manifest,
        &[
            "status",
            "--porcelain",
            "--untracked-files=no",
            "--",
            ".",
            ":(exclude)Cargo.toml",
            ":(exclude)Cargo.lock",
        ],
    )?;
    Some(if status.is_empty() {
        sha
    } else {
        format!("{sha}-dirty")
    })
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

fn run(program: &str, args: &[&str], cwd: &Path) {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|err| {
            panic!("failed to start {program}: {err} (needed to embed git uplink web-ui)")
        });
    if !status.success() {
        panic!("{program} {} failed in {}", args.join(" "), cwd.display());
    }
}
