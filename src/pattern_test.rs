use super::*;

// The expectations are what the regular expression each function replaced
// returned for the same input.

#[test]
fn remove_html_comments_takes_the_shortest_comment_and_keeps_unclosed_ones() {
    for (text, want) in [
        ("a<!-- x -->b", "ab"),
        ("a<!--\nsecret\n-->b<!--c-->d", "abd"),
        ("<!-- one --> keep <!-- two -->", " keep "),
        ("<!-- outer <!-- inner --> tail -->", " tail -->"),
        ("a<!-->b", "a<!-->b"),
        ("a<!--->b", "a<!--->b"),
        ("a<!---->b", "ab"),
        ("open <!-- never closed", "open <!-- never closed"),
        ("x<!--a-->y<!-- unclosed", "xy<!-- unclosed"),
        ("é<!--ü-->ß", "éß"),
    ] {
        assert_eq!(remove_html_comments(text), want, "{text:?}");
    }
}

#[test]
fn collapse_blank_lines_needs_three_breaks_with_only_spaces_and_tabs() {
    for (text, want) in [
        ("a\n\nb", "a\n\nb"),
        ("a\n\n\nb", "a\n\nb"),
        ("a\n \t\n\t \n  \nb", "a\n\nb"),
        ("a\n\n\n  b", "a\n\n  b"),
        ("a\r\n\r\n\r\nb", "a\r\n\r\n\r\nb"),
        ("a\n\u{a0}\n\nb", "a\n\u{a0}\n\nb"),
        ("\n\n\n", "\n\n"),
        ("a\n\n\nb\n\n\n\nc", "a\n\nb\n\nc"),
    ] {
        assert_eq!(collapse_blank_lines(text), want, "{text:?}");
    }
}

#[test]
fn header_lines_match_at_line_starts_in_any_case() {
    let text = "Subject\nuplink-depends-on: upl_a, upl_b\n  Uplink-Depends-On: no\nUPLINK-DEPENDS-ON:upl_c\r\nx Uplink-Depends-On: no";
    let values: Vec<_> = header_lines(text, "Uplink-Depends-On")
        .iter()
        .map(|h| h.value)
        .collect();
    assert_eq!(values, ["upl_a, upl_b", "upl_c\r"]);
}

#[test]
fn header_with_nothing_after_the_colon_takes_the_next_line() {
    let text = "Uplink-Depends-On:\n\n  upl_0123456789\nrest";
    let headers = header_lines(text, "Uplink-Depends-On");
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].value, "upl_0123456789");
    assert_eq!(remove_header_lines(text, "Uplink-Depends-On"), "\nrest");
}

#[test]
fn header_at_the_end_of_the_text_keeps_one_blank_character() {
    let headers = header_lines("Co-Authored-By: \t", "Co-Authored-By");
    assert_eq!(headers[0].value, "\t");
    assert!(header_lines("Co-Authored-By:\n\n", "Co-Authored-By").is_empty());
    assert!(header_lines("Co-Authored-By:", "Co-Authored-By").is_empty());
}

#[test]
fn remove_header_lines_keeps_the_line_breaks_around_them() {
    assert_eq!(
        remove_header_lines(
            "Body\nUplink-Depends-On: upl_x\nMore\nuplink-depends-on: upl_y",
            "Uplink-Depends-On"
        ),
        "Body\n\nMore\n"
    );
}

#[test]
fn is_patch_id_is_upl_and_ten_hex_digits_in_any_case() {
    for (token, want) in [
        ("upl_0123456789", true),
        ("UPL_abcdefABCD", true),
        ("upl_012345678", false),
        ("upl_0123456789a", false),
        ("upl_012345678g", false),
        ("up_l0123456789", false),
        ("upl٣0123456789", false),
        ("", false),
    ] {
        assert_eq!(is_patch_id(token), want, "{token:?}");
    }
}

#[test]
fn split_person_takes_the_last_angle_bracket_address() {
    for (value, want) in [
        (
            "Ann Lee <ann@example.com>",
            Some(("Ann Lee", "ann@example.com")),
        ),
        ("<a@b.c>", None),
        (" <a@b.c>", Some(("", "a@b.c"))),
        ("Ann  \t<a@b.c>", Some(("Ann  ", "a@b.c"))),
        ("Ann <x> <a@b.c>", Some(("Ann <x>", "a@b.c"))),
        ("Ann <a <b>", Some(("Ann <a", "b"))),
        ("Ann\nLee <a@b.c>", None),
        ("Ann \n<a@b.c>", Some(("Ann ", "a@b.c"))),
        ("Ann <>", None),
        ("Ann <a@b.c> trailing", None),
        ("ann@example.com", None),
    ] {
        assert_eq!(split_person(value), want, "{value:?}");
    }
}

#[test]
fn find_tickets_needs_word_boundaries_and_two_to_ten_capitals() {
    for (text, want) in [
        ("Fixes JIRA-12 and AB-3.", vec!["JIRA-12", "AB-3"]),
        ("A-1 ABCDEFGHIJK-1 ABCDEFGHIJ-1", vec!["ABCDEFGHIJ-1"]),
        ("xJIRA-1 JIRA-1x JIRA-1_ _JIRA-1", vec![]),
        ("(JIRA-7),JIRA-8/JIRA-9", vec!["JIRA-7", "JIRA-8", "JIRA-9"]),
        ("éJIRA-1 JIRA-1é", vec![]),
        ("JIRA-", vec![]),
        ("JIRA-٣", vec!["JIRA-٣"]),
        ("jira-12 Jira-12", vec![]),
    ] {
        assert_eq!(find_tickets(text), want, "{text:?}");
    }
}

#[test]
fn github_owner_repo_reads_https_and_ssh_remotes() {
    for (url, want) in [
        ("https://github.com/acme/tool.git", Some(("acme", "tool"))),
        ("https://github.com/acme/tool/", Some(("acme", "tool"))),
        ("https://github.com/acme/tool.git/", Some(("acme", "tool"))),
        ("git@github.com:acme/tool", Some(("acme", "tool"))),
        (
            "https://github.com/acme/group/tool",
            Some(("acme", "group/tool")),
        ),
        ("https://github.com//acme/tool", Some(("/acme", "tool"))),
        ("https://github.com/acme/.git", Some(("acme", ".git"))),
        ("https://github.com/acme", None),
        ("https://github.com/acme/", None),
        ("https://github.com/a/b\n", None),
        ("https://gitlab.com/acme/tool", None),
        (
            "github.com.evil/x github.com/acme/tool",
            Some(("acme", "tool")),
        ),
    ] {
        assert_eq!(github_owner_repo(url), want, "{url:?}");
    }
}

#[test]
fn number_after_needs_digits_ended_by_slash_query_or_end() {
    let pull = ["/pulls/", "/pull/"];
    for (url, want) in [
        ("https://github.com/a/b/pull/99", Some("99")),
        ("https://github.com/a/b/pulls/7/files", Some("7")),
        ("https://github.com/a/b/pull/12?x=1", Some("12")),
        ("https://github.com/a/b/pull/12#c", None),
        ("https://github.com/a/b/pull/x/pull/5", Some("5")),
        ("https://github.com/a/b/pull/", None),
        ("https://github.com/a/b/issues/3", None),
    ] {
        assert_eq!(number_after(url, &pull), want, "{url:?}");
    }
    assert_eq!(number_after("/a/issues/3/", &["/issues/"]), Some("3"));
}

/// The `split_person` that this file replaced: quadratic, kept to check that
/// the linear one returns the same results.
fn quadratic_split_person(value: &str) -> Option<(&str, &str)> {
    let inner = value.strip_suffix('>')?;
    let mut splits: Vec<usize> = value.char_indices().map(|(i, _)| i).collect();
    splits.reverse();
    for name_end in splits {
        let name = &value[..name_end];
        if name.contains('\n') {
            continue;
        }
        let rest = &value[name_end..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(open) = rest.find(|c: char| !c.is_whitespace()) else {
            continue;
        };
        let open = name_end + open;
        if !value[open..].starts_with('<') || open + 1 > inner.len() {
            continue;
        }
        let address = &inner[open + 1..];
        if !address.is_empty() && !address.contains('>') {
            return Some((name, address));
        }
    }
    None
}

/// The `github_owner_repo` that this file replaced, kept for the same reason.
fn quadratic_github_owner_repo(url: &str) -> Option<(&str, &str)> {
    let mut from = 0;
    while let Some(offset) = url[from..].find("github.com") {
        let host = from + offset;
        from = host + 1;
        let sep = host + "github.com".len();
        if !(url[sep..].starts_with(':') || url[sep..].starts_with('/')) {
            continue;
        }
        let tail = &url[sep + 1..];
        if tail.contains('\n') {
            continue;
        }
        for (slash, _) in tail.match_indices('/').filter(|(i, _)| *i > 0) {
            let rest = &tail[slash + 1..];
            if rest.is_empty() {
                continue;
            }
            let name = [".git/", ".git", "/"]
                .iter()
                .find_map(|suffix| rest.strip_suffix(suffix).filter(|name| !name.is_empty()))
                .unwrap_or(rest);
            return Some((&tail[..slash], name));
        }
    }
    None
}

/// Strings of up to `max_len` pieces drawn from `pieces` by a fixed-seed
/// xorshift, so a failure reproduces.
fn generated(pieces: &[&str], max_len: u64, count: usize) -> Vec<String> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..count)
        .map(|_| {
            let len = next() % (max_len + 1);
            (0..len)
                .map(|_| pieces[(next() % pieces.len() as u64) as usize])
                .collect()
        })
        .collect()
}

#[test]
fn split_person_matches_the_quadratic_version() {
    let pieces = [" ", " ", "\t", "\n", "\u{a0}", "<", "<", ">", "a", "é"];
    for mut value in generated(&pieces, 14, 200_000) {
        if !value.ends_with('>') && value.len() % 3 != 0 {
            value.push('>');
        }
        assert_eq!(
            split_person(&value),
            quadratic_split_person(&value),
            "{value:?}"
        );
    }
}

#[test]
fn github_owner_repo_matches_the_quadratic_version() {
    let pieces = ["github.com", ":", "/", "/", "\n", "a", ".git", "x"];
    for url in generated(&pieces, 10, 200_000) {
        assert_eq!(
            github_owner_repo(&url),
            quadratic_github_owner_repo(&url),
            "{url:?}"
        );
    }
}

/// Runs `check` on a 1 MB input and fails if it takes long enough to suggest
/// quadratic time, which would take minutes on this size.
fn assert_fast(check: impl FnOnce()) {
    let started = std::time::Instant::now();
    check();
    let took = started.elapsed();
    assert!(took.as_secs() < 2, "took {took:?}");
}

#[test]
fn split_person_is_linear_on_long_lines() {
    let pairs = "a ".repeat(500_000);
    let brackets = " <".repeat(500_000);
    let angles = "<".repeat(1_000_000);
    assert_fast(|| assert_eq!(split_person(&format!("{pairs}>")), None));
    assert_fast(|| assert_eq!(split_person(&format!("{pairs}\n <>")), None));
    assert_fast(|| assert_eq!(split_person(&format!("\nx{angles}a>")), None));
    assert_fast(|| {
        let value = format!("{brackets}>");
        let want = Some((&brackets[..brackets.len() - 4], " <"));
        assert_eq!(split_person(&value), want);
    });
    assert_fast(|| {
        let value = format!("{pairs}<a>");
        assert_eq!(split_person(&value), Some((&pairs[..pairs.len() - 1], "a")));
    });
}

#[test]
fn github_owner_repo_is_linear_on_long_lines() {
    let hosts = "github.com:".repeat(100_000);
    assert_fast(|| assert_eq!(github_owner_repo(&hosts), None));
    assert_fast(|| assert_eq!(github_owner_repo(&format!("{hosts}\n")), None));
    assert_fast(|| assert_eq!(github_owner_repo(&format!("{hosts}\nx")), None));
    assert_fast(|| {
        let url = format!("{}github.com/acme/tool", "github.comx".repeat(100_000));
        assert_eq!(github_owner_repo(&url), Some(("acme", "tool")));
    });
}
