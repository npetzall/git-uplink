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
    println!("cargo:rerun-if-changed=way-of-working.md");
    println!("cargo:rerun-if-changed=templates/README.md");

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    run(npm, &["ci", "--no-fund", "--no-audit"], &web);
    run(npm, &["run", "build"], &web);
    if !dist.join("index.html").is_file() {
        panic!("vite did not write web/dist/index.html");
    }
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
