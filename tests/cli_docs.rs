//! `docs/cli.md` is generated from the clap definitions in `src/cli.rs`.
//!
//! The test fails when the file is stale. Regenerate it with
//! `UPLINK_BLESS=1 sfw cargo test --locked --test cli_docs`.

use std::fmt::Write as _;
use std::path::Path;

use clap::{Arg, ArgAction, Command};
use git_uplink::cli;

const HEADER: &str = "<!-- Generated from src/cli.rs. Do not edit; run \
`UPLINK_BLESS=1 sfw cargo test --locked --test cli_docs`. -->";
const SYNOPSIS_WIDTH: usize = 88;
const SYNOPSIS_INDENT: &str = "            ";

/// Escapes what Markdown would read as markup, outside code spans and
/// indented code blocks.
fn markdown(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with("    ") {
            out.push_str(line);
        } else {
            let mut code = false;
            for ch in line.chars() {
                match ch {
                    '`' => code = !code,
                    '<' | '*' if !code => out.push('\\'),
                    _ => {}
                }
                out.push(ch);
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn value(arg: &Arg) -> String {
    arg.get_value_names()
        .map(|names| {
            names
                .iter()
                .map(|name| format!("<{name}>"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// `--flag <value>` or `<value>`, as typed.
fn usage(arg: &Arg) -> String {
    let Some(long) = arg.get_long() else {
        return value(arg);
    };
    if arg.get_action().takes_values() {
        format!("--{long} {}", value(arg))
    } else {
        format!("--{long}")
    }
}

fn synopsis(sub: &Command) -> String {
    let grouped = |arg: &Arg| {
        sub.get_groups()
            .find(|group| group.is_required_set() && group.get_args().any(|id| id == arg.get_id()))
    };
    // Options that exclude each other, both ways, share one bracket.
    let excludes = |a: &Arg, b: &Arg| {
        let one_way = |a: &Arg, b: &Arg| {
            sub.get_arg_conflicts_with(a)
                .iter()
                .any(|other| other.get_id() == b.get_id())
        };
        !a.is_positional() && !b.is_positional() && one_way(a, b) && one_way(b, a)
    };
    let mut words = Vec::new();
    let mut seen_groups = Vec::new();
    let mut merged = Vec::new();
    let positionals = sub.get_arguments().filter(|arg| arg.is_positional());
    let options = sub.get_arguments().filter(|arg| !arg.is_positional());
    for arg in positionals.chain(options) {
        if merged.contains(&arg.get_id()) {
            continue;
        }
        if let Some(group) = grouped(arg) {
            if !seen_groups.contains(&group.get_id()) {
                seen_groups.push(group.get_id());
                let members: Vec<String> = sub
                    .get_arguments()
                    .filter(|other| group.get_args().any(|id| id == other.get_id()))
                    .map(usage)
                    .collect();
                words.push(format!("({})", members.join(" | ")));
            }
            continue;
        }
        let mut word = usage(arg);
        for other in sub.get_arguments().filter(|other| excludes(arg, other)) {
            merged.push(other.get_id());
            write!(word, " | {}", usage(other)).unwrap();
        }
        if !arg.is_required_set() {
            word = format!("[{word}]");
        }
        if matches!(arg.get_action(), ArgAction::Append) {
            word.push_str("...");
        }
        words.push(word);
    }

    let mut out = String::new();
    let mut line = format!("git uplink {}", sub.get_name());
    for word in words {
        if line.len() + 1 + word.len() > SYNOPSIS_WIDTH {
            writeln!(out, "{line}").unwrap();
            line = format!("{SYNOPSIS_INDENT}{word}");
        } else {
            line = format!("{line} {word}");
        }
    }
    writeln!(out, "{line}").unwrap();
    out
}

/// One list item per argument: usage, help, default, possible values.
fn argument(arg: &Arg) -> String {
    let help = arg
        .get_long_help()
        .or_else(|| arg.get_help())
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut paragraphs = help.split("\n\n").map(markdown);
    let mut first = paragraphs.next().unwrap_or_default();
    if !first.ends_with('.') {
        first.push('.');
    }
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| format!("`{}`", value.to_string_lossy()))
        .collect();
    if !defaults.is_empty() {
        write!(first, " Default: {}.", defaults.join(", ")).unwrap();
    }

    let mut out = format!("- `{}`: {first}\n", usage(arg));
    for paragraph in paragraphs {
        writeln!(out, "\n  {}", paragraph.replace('\n', "\n  ")).unwrap();
    }
    if arg.get_action().takes_values() {
        for possible in arg.get_possible_values() {
            let help = possible
                .get_help()
                .map(|help| format!(": {}.", markdown(&help.to_string())))
                .unwrap_or_default();
            writeln!(out, "  - `{}`{help}", possible.get_name()).unwrap();
        }
    }
    out
}

fn render() -> String {
    let root = cli::command("");
    let mut out = String::new();
    writeln!(out, "{HEADER}\n\n# CLI reference\n").unwrap();
    let overview = root.get_long_about().unwrap().to_string();
    writeln!(out, "{}\n", markdown(&overview)).unwrap();

    writeln!(out, "## Synopsis\n\n```text").unwrap();
    for sub in root.get_subcommands() {
        out.push_str(&synopsis(sub));
    }
    writeln!(out, "```").unwrap();

    for (heading, names) in cli::GROUPS {
        writeln!(out, "\n## {heading}").unwrap();
        for name in *names {
            let sub = root.find_subcommand(name).unwrap();
            writeln!(out, "\n### `{name}`\n").unwrap();
            let about = sub
                .get_long_about()
                .or_else(|| sub.get_about())
                .unwrap()
                .to_string();
            let end = if about.ends_with('.') || about.contains("\n\n") {
                ""
            } else {
                "."
            };
            writeln!(out, "{}{end}", markdown(&about)).unwrap();
            let mut arguments = sub.get_arguments().peekable();
            if arguments.peek().is_some() {
                out.push('\n');
            }
            for arg in arguments {
                assert!(
                    arg.get_help().is_some(),
                    "{name} {} has no help text",
                    arg.get_id()
                );
                out.push_str(&argument(arg));
            }
        }
    }

    for (title, body) in cli::TOPICS {
        writeln!(out, "\n## {}\n\n{}", markdown(title), markdown(body)).unwrap();
    }
    out
}

#[test]
fn every_command_is_in_exactly_one_group() {
    let mut grouped: Vec<&str> = cli::GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter().copied())
        .collect();
    let mut commands: Vec<&str> = Vec::new();
    let root = cli::command("");
    for sub in root.get_subcommands() {
        commands.push(sub.get_name());
    }
    grouped.sort_unstable();
    commands.sort_unstable();
    assert_eq!(grouped, commands);
}

#[test]
fn cli_md_matches_the_help_text() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/cli.md");
    let rendered = render();
    if std::env::var_os("UPLINK_BLESS").is_some() {
        std::fs::write(&path, &rendered).unwrap();
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == rendered,
        "docs/cli.md is stale; run `UPLINK_BLESS=1 sfw cargo test --locked --test cli_docs`"
    );
}
