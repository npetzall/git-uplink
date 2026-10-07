use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

// The clap definitions, shared with the crate, so the man pages come from the
// same help text. Only `command` is used here.
#[allow(dead_code)]
#[path = "src/cli.rs"]
mod cli;

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
    println!("cargo:rerun-if-changed=templates/github-hooks");
    println!("cargo:rerun-if-changed=templates/try-it-on-github");
    println!("cargo:rerun-if-env-changed=GIT_UPLINK_SKIP_WEB_BUILD");

    let commit = embed_commit(&manifest);
    println!("cargo:rerun-if-changed=src/cli.rs");
    // The clap tree is built unoptimized here and outgrows the 1 MiB main thread
    // stack on Windows.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || generate_man_pages(&commit))
        .unwrap()
        .join()
        .unwrap();

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
fn embed_commit(manifest: &Path) -> String {
    println!("cargo:rerun-if-env-changed=GIT_UPLINK_COMMIT");
    let commit = match env::var("GIT_UPLINK_COMMIT") {
        Ok(commit) if !commit.is_empty() => commit,
        _ => git_commit(manifest).unwrap_or_else(|| "unknown".to_string()),
    };
    println!("cargo:rustc-env=GIT_UPLINK_COMMIT={commit}");
    commit
}

/// Writes `git-uplink.1` and one page per subcommand to `$OUT_DIR/man`, plus
/// `man_pages.rs`, the table `src/man.rs` embeds them through.
fn generate_man_pages(commit: &str) {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let dir = out.join("man");
    // A stale page of a removed subcommand must not be embedded.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let version = format!("{} ({commit})", env::var("CARGO_PKG_VERSION").unwrap());
    let mut command = cli::command(version.leak()).disable_help_subcommand(true);
    command.build();
    clap_mangen::generate_to(command.clone(), &dir).unwrap();
    std::fs::write(dir.join("git-uplink.1"), main_page(command)).unwrap();

    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut table = String::from("pub const MAN_PAGES: &[(&str, &[u8])] = &[\n");
    for name in names {
        table.push_str(&format!(
            "    ({name:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/man/{name}\"))),\n"
        ));
    }
    table.push_str("];\n");
    std::fs::write(out.join("man_pages.rs"), table).unwrap();
}

/// The top-level page, with the commands grouped by `cli::GROUPS` and each of
/// `cli::TOPICS` as a section of its own instead of clap_mangen's single EXTRA
/// section.
fn main_page(command: clap::Command) -> Vec<u8> {
    let man_command = command.clone();
    let man = clap_mangen::Man::new(command);
    let mut page = Vec::new();
    man.render_title(&mut page).unwrap();
    man.render_name_section(&mut page).unwrap();
    man.render_synopsis_section(&mut page).unwrap();
    man.render_description_section(&mut page).unwrap();
    man.render_options_section(&mut page).unwrap();
    let mut commands = roff::Roff::new();
    commands.control("SH", ["SUBCOMMANDS"]);
    for (heading, names) in cli::GROUPS {
        commands.control("SS", [*heading]);
        for name in *names {
            let about = man_command.find_subcommand(name).unwrap().get_about();
            commands.control("TP", []);
            commands.text([roff::roman(format!("git-uplink-{name}(1)"))]);
            commands.text([roff::roman(about.unwrap().to_string())]);
        }
    }
    commands.to_writer(&mut page).unwrap();
    for (title, body) in cli::TOPICS {
        let mut section = roff::Roff::new();
        section.control("SH", [title.to_uppercase().as_str()]);
        for line in body.lines() {
            section.text([roff::roman(line)]);
        }
        section.to_writer(&mut page).unwrap();
    }
    man.render_version_section(&mut page).unwrap();
    page
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
