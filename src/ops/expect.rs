use super::*;

/// The marker a split or merge start writes into its gated PR description,
/// once per patch the gate rewrites: `<!-- Uplink-Gate-Base: <id>=<stable> -->`.
/// It is an HTML comment, so it never reaches a patch message.
pub const GATE_BASE_KEY: &str = "Uplink-Gate-Base";

/// `id=stable` for each of `ids`: what the gate was cut from. A patch with
/// no stable id yet is written `id=-`.
pub(super) fn expect_lines(queue: &QueueState, ids: &[String]) -> Result<Vec<String>> {
    ids.iter()
        .map(|id| {
            let patch = get_patch(queue, id)?;
            Ok(format!(
                "{id}={}",
                patch.patch_id_stable.as_deref().unwrap_or("-")
            ))
        })
        .collect()
}

/// The HTML comments that carry `expect` in a gated PR description.
pub fn gate_base_comments(expect: &[String]) -> String {
    expect
        .iter()
        .map(|line| format!("<!-- {GATE_BASE_KEY}: {line} -->\n"))
        .collect()
}

/// Parses `id=stable` pairs, in order.
pub(super) fn parse_expect(expect: &[String]) -> Result<Vec<(String, String)>> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for line in expect {
        let Some((id, stable)) = line.trim().split_once('=') else {
            return Err(Error::msg(format!(
                "--expect takes <id>=<stable patch id>, not {line}"
            )));
        };
        let (id, stable) = (id.trim(), stable.trim());
        if id.is_empty() || stable.is_empty() {
            return Err(Error::msg(format!(
                "--expect takes <id>=<stable patch id>, not {line}"
            )));
        }
        if pairs.iter().any(|(seen, _)| seen == id) {
            return Err(Error::msg(format!("--expect names {id} twice")));
        }
        pairs.push((id.to_string(), stable.to_string()));
    }
    Ok(pairs)
}

/// Refuses when a patch the gate was cut from changed since: another
/// rewrite of it was recorded, or it is no longer active. The developer
/// dispatches the gate again on what the queue has now.
pub(super) fn check_expected(queue: &QueueState, pairs: &[(String, String)]) -> Result<()> {
    for (id, stable) in pairs {
        let patch = get_patch(queue, id)?;
        let now = patch.patch_id_stable.as_deref().unwrap_or("-");
        if !patch.status.is_active() || now != stable {
            return Err(Error::msg(format!(
                "{id} changed after this PR was opened (it is {} now); close this PR and dispatch it again",
                patch.status
            )));
        }
    }
    Ok(())
}
