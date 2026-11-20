//! Hand-written matchers for the few fixed text patterns uplink needs. Each
//! states the regular expression it replaces and keeps its matches: the
//! leftmost match, with the same greedy or lazy choices. One approximation:
//! `\d` and `\w` use `char::is_numeric` and `char::is_alphanumeric`, which
//! also count other numerals (`²`, `½`) and leave out combining marks.

/// `(?s)<!--.*?-->` replaced by nothing. An unclosed comment is kept.
pub fn remove_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("<!--") {
        let Some(close) = rest[open + 4..].find("-->") else {
            break;
        };
        out.push_str(&rest[..open]);
        rest = &rest[open + 4 + close + 3..];
    }
    out.push_str(rest);
    out
}

/// `\n[ \t]*\n(?:[ \t]*\n)+` replaced by `\n\n`: three or more line breaks
/// with only spaces and tabs between them become one blank line.
pub fn collapse_blank_lines(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\n' {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        let mut breaks = 0;
        loop {
            let mut k = end;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b'\n' {
                breaks += 1;
                end = k + 1;
            } else {
                break;
            }
        }
        if breaks >= 2 {
            out.push_str(&text[copied..i]);
            out.push_str("\n\n");
            copied = end;
            i = end;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// A match of [`header_lines`]: the byte range of the whole match and its
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header<'a> {
    pub start: usize,
    pub end: usize,
    pub value: &'a str,
}

/// Every match of `(?im)^{name}:\s*(.+)$`, in order. The name is matched
/// without regard to ASCII case. As in the regex, `\s*` crosses line breaks,
/// so a header with nothing after the colon takes the next non-blank line as
/// its value.
pub fn header_lines<'a>(text: &'a str, name: &str) -> Vec<Header<'a>> {
    let mut found = Vec::new();
    let mut line = 0;
    while line <= text.len() {
        let matched = header_at(text, line, name);
        let resume = match matched {
            Some(header) => {
                found.push(header);
                header.end
            }
            None => line,
        };
        match text[resume..].find('\n') {
            Some(offset) => line = resume + offset + 1,
            None => break,
        }
    }
    found
}

fn header_at<'a>(text: &'a str, start: usize, name: &str) -> Option<Header<'a>> {
    let label = text.get(start..start + name.len())?;
    if !label.eq_ignore_ascii_case(name) || !text[start + name.len()..].starts_with(':') {
        return None;
    }
    let after = start + name.len() + 1;
    let skipped = text[after..]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(text.len(), |(offset, _)| after + offset);
    // `.+` needs one character that is not a line break. At the end of the
    // text, `\s*` gives back its last such character.
    let value_start = if skipped < text.len() {
        skipped
    } else {
        let (offset, _) = text[after..]
            .char_indices()
            .rev()
            .find(|(_, c)| *c != '\n')?;
        after + offset
    };
    let end = text[value_start..]
        .find('\n')
        .map_or(text.len(), |offset| value_start + offset);
    Some(Header {
        start,
        end,
        value: &text[value_start..end],
    })
}

/// `text` without the matches of [`header_lines`].
pub fn remove_header_lines(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for header in header_lines(text, name) {
        out.push_str(&text[copied..header.start]);
        copied = header.end;
    }
    out.push_str(&text[copied..]);
    out
}

/// `(?i)^upl_[0-9a-f]{10}$`.
pub fn is_patch_id(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() == 14
        && bytes[..4].eq_ignore_ascii_case(b"upl_")
        && bytes[4..].iter().all(u8::is_ascii_hexdigit)
}

/// The two groups of `^(.*)\s+<([^>]+)>$`: the name and the address of
/// `Name <address>`, untrimmed.
pub fn split_person(value: &str) -> Option<(&str, &str)> {
    let inner = value.strip_suffix('>')?;
    // `[^>]+` keeps the `<` after the last `>` of `inner`.
    let floor = inner.rfind('>').map_or(0, |i| i + 1);
    // `.` stops at a line break, so the name ends at the first one at the latest.
    let newline = value.find('\n').unwrap_or(value.len());
    // `(.*)` is greedy, so the latest `<` with a split that works wins. Each
    // `<` looks back only over its own run of whitespace, which keeps this
    // linear.
    let mut end = inner.len();
    while let Some(offset) = inner[floor..end].rfind('<') {
        let open = floor + offset;
        end = open;
        let address = &inner[open + 1..];
        let gap_start = inner[..open].trim_end_matches(char::is_whitespace).len();
        if address.is_empty() || gap_start == open {
            continue;
        }
        // `\s+` keeps at least the last whitespace character.
        let last_space = inner[..open]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        let name_end = last_space.min(newline);
        if name_end >= gap_start {
            return Some((&value[..name_end], address));
        }
    }
    None
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every match of `\b[A-Z]{2,10}-\d+\b`, in order.
pub fn find_tickets(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or_default();
        if c.is_ascii_uppercase()
            && !prev.is_some_and(is_word)
            && let Some(end) = ticket_end(text, i)
        {
            found.push(&text[i..end]);
            prev = text[..end].chars().next_back();
            i = end;
            continue;
        }
        prev = Some(c);
        i += c.len_utf8();
    }
    found
}

fn ticket_end(text: &str, start: usize) -> Option<usize> {
    let letters = text[start..]
        .bytes()
        .take_while(u8::is_ascii_uppercase)
        .count();
    if !(2..=10).contains(&letters) || !text[start + letters..].starts_with('-') {
        return None;
    }
    let digits_start = start + letters + 1;
    let digits = text[digits_start..]
        .char_indices()
        .find(|(_, c)| !c.is_numeric())
        .map_or(text.len() - digits_start, |(offset, _)| offset);
    let end = digits_start + digits;
    let boundary = !text[end..].chars().next().is_some_and(is_word);
    (digits > 0 && boundary).then_some(end)
}

/// The two groups of `github\.com[:/](.+?)/(.+?)(?:\.git)?/?$`: owner and
/// repository name.
pub fn github_owner_repo(url: &str) -> Option<(&str, &str)> {
    // `.` stops at a line break and `$` is the end of the text, so a match
    // starts after the last line break.
    let mut from = url.rfind('\n').map_or(0, |i| i + 1);
    while let Some(offset) = url[from..].find("github.com") {
        let host = from + offset;
        from = host + 1;
        let sep = host + "github.com".len();
        if !(url[sep..].starts_with(':') || url[sep..].starts_with('/')) {
            continue;
        }
        let tail = &url[sep + 1..];
        // A later `github.com` offers only a suffix of this tail, which has
        // no owner and name split that this one lacks, so the first decides.
        let (slash, _) = tail
            .match_indices('/')
            .find(|&(i, _)| i > 0 && i + 1 < tail.len())?;
        let rest = &tail[slash + 1..];
        // `(.+?)` is lazy, so the name drops the longest optional suffix.
        let name = [".git/", ".git", "/"]
            .iter()
            .find_map(|suffix| rest.strip_suffix(suffix).filter(|name| !name.is_empty()))
            .unwrap_or(rest);
        return Some((&tail[..slash], name));
    }
    None
}

/// The digits of the first match of `{prefix}(\d+)(?:/|$|\?)` for any of
/// `prefixes`, as in `/pulls?/(\d+)(?:/|$|\?)`.
pub fn number_after<'a>(url: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for (start, _) in url.char_indices() {
        for prefix in prefixes {
            if !url[start..].starts_with(prefix) {
                continue;
            }
            let digits_start = start + prefix.len();
            let digits = url[digits_start..]
                .char_indices()
                .find(|(_, c)| !c.is_numeric())
                .map_or(url.len() - digits_start, |(offset, _)| offset);
            let end = digits_start + digits;
            let terminated = matches!(url[end..].chars().next(), None | Some('/' | '?'));
            if digits > 0 && terminated {
                return Some(&url[digits_start..end]);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "pattern_test.rs"]
mod tests;
