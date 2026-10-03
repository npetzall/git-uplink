//! The clap definitions, and nothing else: `build.rs` includes this file to
//! generate the man pages, so it can only depend on clap and std.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

/// The parser with `version` filled in. The version carries the build commit,
/// which `build.rs` computes, so it is passed in instead of read here.
pub fn command(version: &'static str) -> clap::Command {
    Cli::command().version(version)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ForgeArg {
    /// The base GitHub pack, for github.com and GitHub Enterprise Cloud.
    #[value(alias = "ghec")]
    Github,
    /// The base pack adjusted for the worked example.
    #[value(alias = "example-github")]
    TryItOnGithub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum MergeViaArg {
    Pr,
    Trailer,
    PatchId,
    EmptyRebase,
    Manual,
}

#[derive(Parser)]
#[command(
    name = "git-uplink",
    bin_name = "git uplink",
    about = "Carry internal patches on upstream, contribute once, drop when merged.",
    long_about = "Developers open PRs and merge them; they never push main.\n\
add records a merged PR as a queued patch on uplink/state. assess uses the PR\n\
title and body as the single commit message, adds a co-author trailer, strips\n\
the internal section before contrib export, and scans for company affiliation.\n\
On GitHub Enterprise Cloud, contribution approval is the to-upstream Environment;\n\
approve/submit run after that review. git uplink talks to git only; workflows\n\
use gh for GitHub and follow-up commands (submitted, gated) to record results."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Init {
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        contrib: Option<String>,
        #[arg(long = "upstream-remote-name")]
        upstream_remote_name: Option<String>,
        #[arg(long = "upstream-branch")]
        upstream_branch: Option<String>,
        #[arg(long = "contrib-remote-name")]
        contrib_remote_name: Option<String>,
        #[arg(long = "internal-branch")]
        internal_branch: Option<String>,
        #[arg(long, value_enum)]
        forge: Option<ForgeArg>,
        #[arg(long)]
        upgrade: bool,
        #[arg(
            long = "adopt-groups",
            help = "JSON file of commit groups when internal is ahead of upstream"
        )]
        adopt_groups: Option<PathBuf>,
        #[arg(long, help = "Print queue config as JSON")]
        json: bool,
        #[arg(
            long,
            help = "Command a new preflight.sh on uplink/hooks starts with (asked in a terminal when omitted)"
        )]
        preflight: Option<String>,
        #[arg(
            long = "redact-keyword",
            value_name = "WORD",
            help = "uplink.toml: word that must not appear in a contribution (repeatable, or comma-separated)"
        )]
        redact_keyword: Option<Vec<String>>,
        #[arg(
            long = "internal-domain",
            value_name = "DOMAIN",
            help = "uplink.toml: internal email domain to flag in the export (repeatable, or comma-separated)"
        )]
        internal_domain: Option<Vec<String>>,
    },
    Add {
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long, help = "Base revision (fetched from origin if missing)")]
        from: Option<String>,
        #[arg(long, help = "Head revision (fetched from origin if missing)")]
        head: Option<String>,
        #[arg(long)]
        internal_only: bool,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long)]
        pr_url: Option<String>,
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(
            long = "extra-dir",
            help = "Directory of company assessment-hook *.md extras to store with the patch"
        )]
        extra_dir: Option<PathBuf>,
        #[arg(
            long = "extra-source",
            requires = "extra_dir",
            help = "Where the extras came from, such as the hook run URL"
        )]
        extra_source: Option<String>,
    },
    /// Publish local uplink/state, restacking unique patches if origin moved.
    Push {
        #[arg(long = "push-remote")]
        push_remote: Option<String>,
    },
    /// Fetch origin tracking refs without moving local branches.
    Refresh,
    /// Fetch origin and hard-reset company main, uplink/state, and uplink/upstream.
    Reset,
    Preflight {
        id: Option<String>,
        #[arg(long, help = "Base revision (fetched from origin if missing)")]
        from: Option<String>,
        #[arg(long, help = "Head revision (fetched from origin if missing)")]
        head: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        #[arg(long)]
        internal_only: bool,
        #[arg(
            long = "command-only",
            conflicts_with_all = ["id", "from", "head"],
            help = "Only run preflight.sh in the current tree"
        )]
        command_only: bool,
        #[arg(
            long,
            value_name = "REV",
            help = "Read preflight.sh from this revision instead of uplink/hooks, to try a change to it"
        )]
        hooks: Option<String>,
    },
    Assess {
        #[arg(long, help = "Base revision (fetched from origin if missing)")]
        from: Option<String>,
        #[arg(long, help = "Head revision (fetched from origin if missing)")]
        head: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(long = "message-file", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        #[arg(long)]
        internal_only: bool,
        #[arg(
            long,
            conflicts_with = "internal_only",
            help = "Assess a queued patch in its layer, with its stored title and message unless --title or --message[-file] is given (for example a conflict resolution or amend)"
        )]
        patch: Option<String>,
    },
    Report {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(
            long = "extra-dir",
            help = "Directory of *.md files prepended to the packet. Without it, extras stored for the unchanged patch are used"
        )]
        extra_dir: Option<PathBuf>,
        #[arg(
            long = "store-extras",
            requires = "extra_dir",
            help = "Also store --extra-dir as the patch's extras for later packets"
        )]
        store_extras: bool,
        #[arg(
            long = "extra-source",
            requires = "store_extras",
            help = "Where the stored extras came from, such as the hook run URL"
        )]
        extra_source: Option<String>,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long, help = "Print doctor report as JSON")]
        json: bool,
    },
    Approve {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Build the export commit on uplink/upstream. The forge creates the
    /// signed contrib commit from the printed `contribCommit`.
    Submit {
        id: String,
        /// Force-push the local, unsigned export commit to contrib instead.
        #[arg(long)]
        push: bool,
    },
    Submitted {
        id: String,
        #[arg(long = "pr-url")]
        pr_url: String,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long = "push-remote", default_value = "origin")]
        push_remote: String,
    },
    Sync,
    /// Promote a pending public main after from-upstream environment approval.
    #[command(name = "accept-upstream")]
    AcceptUpstream,
    /// Record the company PR that gates a conflict.
    #[command(alias = "conflicted")]
    Gated {
        id: String,
        #[arg(long = "pr-url")]
        pr_url: String,
        #[arg(long)]
        pr: Option<u64>,
        #[arg(long = "push-remote", default_value = "origin")]
        push_remote: String,
    },
    Merged {
        id: String,
        #[arg(long, value_enum, default_value_t = MergeViaArg::Manual)]
        via: MergeViaArg,
        #[arg(long)]
        sha: Option<String>,
    },
    Drop {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Rebuild {
        #[arg(
            long,
            help = "Rebuild onto uplink/preview/<name> instead of company main (preview; does not mutate the queue or push)"
        )]
        branch: Option<String>,
        #[arg(long, help = "Push uplink/state and the rebuilt branch after rebuild")]
        push: bool,
        #[arg(long = "push-remote")]
        push_remote: Option<String>,
    },
    Resolve {
        id: String,
    },
    /// Move a patch between the internal and upstream queues.
    #[command(group(
        clap::ArgGroup::new("direction")
            .required(true)
            .args(["to_upstream", "to_internal"])
    ))]
    Transfer {
        id: String,
        #[arg(long = "to-upstream")]
        to_upstream: bool,
        #[arg(long = "to-internal")]
        to_internal: bool,
        #[arg(long, help = "Finish a gated transfer after the work PR is merged")]
        complete: bool,
    },
    /// Revise a patch through a gated PR from uplink/amend/<id>-work.
    Amend {
        id: String,
        #[arg(long, help = "Fold the merged amend PR into the patch")]
        complete: bool,
        #[arg(long, requires = "complete", help = "New patch title")]
        title: Option<String>,
        #[arg(long, requires = "title", conflicts_with = "message_file")]
        message: Option<String>,
        #[arg(
            long = "message-file",
            requires = "title",
            help = "New commit message, as the PR title and body (- reads stdin)"
        )]
        message_file: Option<PathBuf>,
    },
    /// Start the embedded operator dashboard on 127.0.0.1 and open a browser.
    #[command(name = "web-ui")]
    WebUi {
        #[arg(long, default_value_t = 43721)]
        port: u16,
        /// Do not launch a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Print the git uplink version and the commit it was built from.
    Version,
}
