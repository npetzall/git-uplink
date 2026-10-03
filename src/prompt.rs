//! Plain line prompts for `init`. Questions go to stderr so stdout stays
//! machine-readable.

use std::io::{BufRead, Write};

use crate::error::Result;

/// Ask one question. An empty answer keeps `default`.
pub fn ask_line(question: &str, default: &str) -> Result<String> {
    let mut err = std::io::stderr();
    if default.is_empty() {
        write!(err, "{question}: ")?;
    } else {
        write!(err, "{question} [{default}]: ")?;
    }
    err.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}
