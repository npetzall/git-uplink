use super::*;

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub queue: QueueState,
    pub needs_approval: bool,
    pub pending_sha: Option<String>,
    pub flowed_back: Vec<String>,
    pub foreign_commits: Vec<String>,
    /// Patches found merged in the range, with the commit that proves it.
    pub merges: Vec<PendingMerge>,
    /// Commits that name a patch in a trailer but do not match its content.
    pub claims: Vec<IncomingClaim>,
    pub report_path: Option<String>,
    pub report: Option<String>,
}

impl SyncResult {
    fn applied(queue: QueueState) -> Self {
        Self {
            queue,
            needs_approval: false,
            pending_sha: None,
            flowed_back: Vec::new(),
            foreign_commits: Vec::new(),
            merges: Vec::new(),
            claims: Vec::new(),
            report_path: None,
            report: None,
        }
    }

    fn applied_with(queue: QueueState, merges: Vec<PendingMerge>) -> Self {
        Self {
            queue,
            needs_approval: false,
            pending_sha: None,
            flowed_back: merged_ids(&merges),
            foreign_commits: Vec::new(),
            merges,
            claims: Vec::new(),
            report_path: None,
            report: None,
        }
    }
}

/// What the caller already knows about the public pull requests.
#[derive(Debug, Clone, Default)]
pub struct SyncOpts {
    /// `(patch id, merge commit)` for each recorded public PR the forge
    /// reports as merged.
    pub merged_prs: Vec<(String, String)>,
}

/// An upstream commit whose trailer names a patch it does not match.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IncomingClaim {
    pub sha: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommitClass {
    /// Proven to be the patch with this id.
    Ours(String),
    /// Names this patch in a trailer; the content differs.
    Claims(String),
    Foreign,
}

pub(super) struct IncomingCommit {
    sha: String,
    class: CommitClass,
}

/// The pending range, split into what our patches explain and what is left.
pub(super) struct Accounting {
    pending_sha: String,
    from_sha: Option<String>,
    merges: Vec<PendingMerge>,
    commits: Vec<IncomingCommit>,
    /// Diff from (`from_sha` + merged patches) to `pending_sha`. `None` when
    /// the trees are equal: nothing is left to review.
    residual: Option<Residual>,
}

pub(super) struct Residual {
    stat: String,
    diff: String,
}

impl Accounting {
    fn claims(&self) -> Vec<IncomingClaim> {
        self.commits
            .iter()
            .filter_map(|commit| match &commit.class {
                CommitClass::Claims(id) => Some(IncomingClaim {
                    sha: commit.sha.clone(),
                    id: id.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Commits not proven to be one of our patches.
    fn foreign(&self) -> Vec<String> {
        self.commits
            .iter()
            .filter(|commit| !matches!(commit.class, CommitClass::Ours(_)))
            .map(|commit| commit.sha.clone())
            .collect()
    }
}

fn merged_ids(merges: &[PendingMerge]) -> Vec<String> {
    merges.iter().map(|merge| merge.id.clone()).collect()
}

pub(super) fn eligible_for_flow_back(queue: &QueueState, patch: &Patch) -> bool {
    patch.status.is_active() && queue.is_upstream(&patch.id)
}

pub(super) fn commit_stable_patch_id(repo: &Path, sha: &str) -> Result<Option<String>> {
    let shown = git(repo, &["show", "--binary", sha], GitOpts::allow_fail())?;
    if shown.code != 0 || shown.stdout.trim().is_empty() {
        return Ok(None);
    }
    let ident = git(
        repo,
        &["patch-id", "--stable"],
        GitOpts {
            input: Some(shown.stdout.as_bytes()),
            ..GitOpts::default()
        },
    )?;
    Ok(ident
        .stdout
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string))
}

/// The patch a commit is, or only says it is.
///
/// Patch ids are public, so a trailer proves nothing: a commit is ours only
/// when its `git patch-id --stable` equals the patch's. The trailer then
/// decides the label. A trailer on any other diff is a claim.
pub(super) fn classify_commit(repo: &Path, queue: &QueueState, sha: &str) -> Result<CommitClass> {
    let message = git_ok(repo, &["log", "-1", "--format=%B", sha]).unwrap_or_default();
    let named = queue
        .all_patches()
        .filter(|p| eligible_for_flow_back(queue, p))
        .find(|patch| {
            let trailer = format!("{}: {}", queue.config.trailer_key, patch.id);
            message.lines().any(|line| line.trim() == trailer)
        });
    if let Some(stable) = commit_stable_patch_id(repo, sha)? {
        let matches = |patch: &&Patch| patch.patch_id_stable.as_deref() == Some(stable.as_str());
        if let Some(patch) = named.filter(matches).or_else(|| {
            queue
                .all_patches()
                .filter(|p| eligible_for_flow_back(queue, p))
                .find(matches)
        }) {
            return Ok(CommitClass::Ours(patch.id.clone()));
        }
    }
    Ok(match named {
        Some(patch) => CommitClass::Claims(patch.id.clone()),
        None => CommitClass::Foreign,
    })
}

fn has_trailer(repo: &Path, queue: &QueueState, sha: &str, id: &str) -> bool {
    let message = git_ok(repo, &["log", "-1", "--format=%B", sha]).unwrap_or_default();
    let trailer = format!("{}: {id}", queue.config.trailer_key);
    message.lines().any(|line| line.trim() == trailer)
}

/// Splits `from_sha..pending_sha` into merged patches and unaccounted changes.
///
/// A patch is merged when a commit in the range has its stable patch id, or
/// when `opts` reports its public PR merged at a commit in the range. The
/// merged patches are then applied on `from_sha`; whatever still differs
/// from `pending_sha` is the residual a reviewer has to approve.
pub(super) fn account_incoming(
    repo: &Path,
    queue: &QueueState,
    from_sha: Option<&str>,
    pending_sha: &str,
    opts: &SyncOpts,
) -> Result<Accounting> {
    let mut accounting = Accounting {
        pending_sha: pending_sha.to_string(),
        from_sha: from_sha.map(str::to_string),
        merges: Vec::new(),
        commits: Vec::new(),
        residual: None,
    };
    let Some(from) = from_sha else {
        return Ok(accounting);
    };
    let list = git_ok(
        repo,
        &["rev-list", "--reverse", &format!("{from}..{pending_sha}")],
    )?;
    for sha in list.lines().filter(|s| !s.is_empty()) {
        let class = classify_commit(repo, queue, sha)?;
        if let CommitClass::Ours(id) = &class
            && !accounting.merges.iter().any(|merge| &merge.id == id)
        {
            accounting.merges.push(PendingMerge {
                id: id.clone(),
                sha: sha.to_string(),
                via: if has_trailer(repo, queue, sha, id) {
                    MergeVia::Trailer
                } else {
                    MergeVia::PatchId
                },
                modified: false,
            });
        }
        accounting.commits.push(IncomingCommit {
            sha: sha.to_string(),
            class,
        });
    }
    for (id, sha) in &opts.merged_prs {
        add_merged_pr(repo, queue, &mut accounting, id, sha)?;
    }
    // A claim is moot once the patch is proven merged some other way.
    for commit in &mut accounting.commits {
        if let CommitClass::Claims(id) = &commit.class
            && accounting.merges.iter().any(|merge| &merge.id == id)
        {
            commit.class = CommitClass::Foreign;
        }
    }
    accounting.residual = residual(repo, queue, from, pending_sha, &accounting.merges)?;
    Ok(accounting)
}

/// Records a public PR the forge reports as merged at `sha`. Evidence that
/// does not fit the queue or the range is skipped with a note, so a stale
/// report never fails the sync.
fn add_merged_pr(
    repo: &Path,
    queue: &QueueState,
    accounting: &mut Accounting,
    id: &str,
    sha: &str,
) -> Result<()> {
    let skip = |why: &str| eprintln!("ignoring --merged-pr {id}={sha}: {why}");
    let Some(patch) = queue.all_patches().find(|p| p.id == id) else {
        skip("unknown patch");
        return Ok(());
    };
    if !eligible_for_flow_back(queue, patch) {
        skip("not an active upstream patch");
        return Ok(());
    }
    if patch.upstream.as_ref().and_then(|u| u.pr_number).is_none() {
        skip("the patch has no recorded public PR");
        return Ok(());
    }
    if accounting.merges.iter().any(|merge| merge.id == id) {
        return Ok(());
    }
    let resolved = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{sha}^{{commit}}"),
        ],
        GitOpts::allow_fail(),
    )?;
    let Some(commit) = accounting
        .commits
        .iter_mut()
        .find(|commit| resolved.code == 0 && commit.sha == resolved.stdout)
    else {
        skip("the commit is not in the range being synced");
        return Ok(());
    };
    let contained = patch_already_applied_on(repo, &commit.sha, &repo.join(patch_path(id)?))?;
    commit.class = CommitClass::Ours(id.to_string());
    accounting.merges.push(PendingMerge {
        id: id.to_string(),
        sha: commit.sha.clone(),
        via: MergeVia::Pr,
        modified: !contained,
    });
    Ok(())
}

/// Applies `merges` on `from` in queue order and diffs the result against
/// `pending_sha`. A patch that does not apply is left out, so its upstream
/// version shows in the diff in full.
fn residual(
    repo: &Path,
    queue: &QueueState,
    from: &str,
    pending_sha: &str,
    merges: &[PendingMerge],
) -> Result<Option<Residual>> {
    let worktree = TempWorktree::add(repo, "uplink-incoming", from)?;
    let dir = worktree.dir.as_path();
    for patch in apply_order_upstream_layer(queue)? {
        if !merges.iter().any(|merge| merge.id == patch.id) {
            continue;
        }
        let file = repo.join(patch_path(&patch.id)?);
        if apply_abs(dir, &file, &patch.title)? == ApplyOutcome::Conflict {
            git(dir, &["reset", "--hard", "--quiet"], GitOpts::default())?;
            git(dir, &["clean", "-fdq"], GitOpts::default())?;
        }
    }
    let synthetic = git_ok(dir, &["rev-parse", "HEAD^{tree}"])?;
    drop(worktree);
    if synthetic == git_ok(repo, &["rev-parse", &format!("{pending_sha}^{{tree}}")])? {
        return Ok(None);
    }
    Ok(Some(Residual {
        stat: git_ok(repo, &["diff", "--stat", &synthetic, pending_sha])?,
        diff: git_ok(repo, &["diff", "--binary", &synthetic, pending_sha])?,
    }))
}

pub(super) fn write_incoming_packet(
    repo: &Path,
    queue: &QueueState,
    accounting: &Accounting,
) -> Result<String> {
    let title = |id: &str| {
        queue
            .all_patches()
            .find(|p| p.id == id)
            .map(|p| p.title.clone())
            .unwrap_or_default()
    };
    let merges: Vec<IncomingMergeRow> = accounting
        .merges
        .iter()
        .map(|merge| IncomingMergeRow {
            id: merge.id.clone(),
            title: title(&merge.id),
            via: merge.via,
            sha: merge.sha.clone(),
            modified: merge.modified,
        })
        .collect();
    let mut commits = Vec::new();
    for commit in &accounting.commits {
        let described = git_ok(
            repo,
            &["log", "-1", "--format=%an <%ae>%x09%s", &commit.sha],
        )
        .unwrap_or_default();
        let (author, subject) = described.split_once('\t').unwrap_or((&described, ""));
        commits.push(IncomingCommitRow {
            sha: commit.sha.clone(),
            author: author.to_string(),
            subject: subject.to_string(),
            class: match &commit.class {
                CommitClass::Ours(id) => format!("ours: `{id}`"),
                CommitClass::Claims(id) => format!("claims `{id}`, content differs"),
                CommitClass::Foreign => "foreign".into(),
            },
        });
    }
    let residual = accounting.residual.as_ref();
    Ok(format_incoming_packet(&IncomingPacket {
        from_sha: accounting.from_sha.as_deref(),
        pending_sha: &accounting.pending_sha,
        trailer_key: &queue.config.trailer_key,
        merges: &merges,
        claims: &accounting.claims(),
        commits: &commits,
        stat: residual.map(|r| r.stat.as_str()).unwrap_or(""),
        diff: residual.map(|r| r.diff.as_str()).unwrap_or(""),
    }))
}

pub(super) fn restore_company_branch(repo: &Path, company_branch: &str) -> Result<()> {
    git(
        repo,
        &["checkout", "-f", "--quiet", company_branch],
        GitOpts::default(),
    )?;
    Ok(())
}

pub(super) fn persist_apply_conflict(
    repo: &Path,
    queue: &mut QueueState,
    snapshot: &Path,
    company_branch: &str,
    upstream_ref: &str,
    patch: &Patch,
    files: Vec<String>,
) -> Result<ConflictError> {
    let onto = rev_parse(repo, "HEAD")?;
    let (branch, work) = cut_gated_work(
        repo,
        GateKind::Conflict,
        &patch.id,
        &onto,
        &format!("uplink: conflict applying {}", patch.id),
    )?;

    let message = format!(
        "Patch {} (\"{}\") does not apply onto the current upstream prefix.",
        patch.id, patch.title
    );
    {
        let current = get_patch_mut(queue, &patch.id)?;
        current.status = PatchStatus::Conflict;
        current.conflict = Some(PatchConflict {
            branch: branch.clone(),
            work_branch: Some(work),
            files: files.clone(),
            message: message.clone(),
            onto: Some(onto),
            pr_number: None,
            pr_url: None,
        });
        add_event(
            current,
            "conflict",
            if files.is_empty() {
                "untracked conflict".into()
            } else {
                files.join(", ")
            },
        );
    }
    queue.last_sync = Some(LastSync {
        at: stamp(),
        upstream_sha: rev_parse(repo, upstream_ref)?,
        result: "conflict".into(),
        message: Some(message.clone()),
    });

    restore_company_branch(repo, company_branch)?;
    fs::create_dir_all(repo.join(".uplink/patches"))?;
    copy_dir(&snapshot.join(".uplink"), &repo.join(".uplink"))?;
    write_queue_file(repo, queue)?;
    commit_queue(repo, &format!("uplink: conflict on {}", patch.id))?;
    Ok(ConflictError::new(message, &patch.id, files))
}

/// Moves `uplink/upstream` to `sha`, marks `merges` merged, and rebuilds.
/// The rebuild still marks an upstream patch that applies empty.
pub(super) fn apply_fetched_upstream(
    repo: &Path,
    sha: &str,
    merges: &[PendingMerge],
) -> Result<QueueState> {
    promote_upstream(repo, sha)?;
    let mut merged = Vec::new();
    for merge in merges {
        let queue = read_queue_file(repo)?;
        let active = queue
            .all_patches()
            .any(|p| p.id == merge.id && eligible_for_flow_back(&queue, p));
        if active {
            mark_merged(repo, &merge.id, merge.via, Some(&merge.sha))?;
            merged.push(merge.id.clone());
        }
    }
    match rebuild(repo) {
        Ok(mut queue) => {
            queue.pending_upstream = None;
            queue.last_sync = Some(LastSync {
                at: stamp(),
                upstream_sha: sha.to_string(),
                result: "ok".into(),
                message: Some(if merged.is_empty() {
                    "Synced with upstream".into()
                } else {
                    format!("Marked merged: {}", merged.join(", "))
                }),
            });
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: sync with upstream")?;
            Ok(queue)
        }
        Err(Error::Conflict(_)) => {
            let mut queue = read_queue_file(repo)?;
            if queue.pending_upstream.is_some() {
                queue.pending_upstream = None;
                write_queue_file(repo, &queue)?;
                commit_queue(repo, "uplink: clear pending upstream")?;
            }
            Ok(queue)
        }
        Err(err) => Err(err),
    }
}

pub fn sync(repo: &Path) -> Result<SyncResult> {
    sync_with(repo, SyncOpts::default())
}

pub fn sync_with(repo: &Path, opts: SyncOpts) -> Result<SyncResult> {
    with_queue_lock(repo, || {
        let fetched = read_queue_file(repo)?;
        let previous = fetched
            .last_sync
            .as_ref()
            .map(|sync| sync.upstream_sha.clone());
        ensure_upstream_ref(repo)?;
        let from_sha = if has_ref(repo, "uplink/upstream")? {
            Some(rev_parse(repo, "uplink/upstream")?)
        } else {
            None
        };
        let sha = fetch_upstream_remote(repo, &fetched)?;
        if previous.as_deref() == Some(sha.as_str()) && from_sha.as_deref() == Some(sha.as_str()) {
            let mut queue = read_queue_file(repo)?;
            queue.pending_upstream = None;
            queue.last_sync = Some(LastSync {
                at: stamp(),
                upstream_sha: sha,
                result: "ok".into(),
                message: Some("Synced with upstream".into()),
            });
            write_queue_file(repo, &queue)?;
            commit_queue(repo, "uplink: sync with upstream")?;
            return Ok(SyncResult::applied(queue));
        }
        let accounting = account_incoming(repo, &fetched, from_sha.as_deref(), &sha, &opts)?;
        if accounting.residual.is_none() {
            let queue = apply_fetched_upstream(repo, &sha, &accounting.merges)?;
            return Ok(SyncResult::applied_with(queue, accounting.merges));
        }
        let report = write_incoming_packet(repo, &fetched, &accounting)?;
        let report_path = from_upstream_report_paths().1;
        let abs = repo.join(&report_path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut body = report.clone();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        fs::write(&abs, body)?;
        let mut queue = read_queue_file(repo)?;
        queue.pending_upstream = Some(PendingUpstream {
            sha: accounting.pending_sha.clone(),
            from_sha: accounting.from_sha.clone(),
            at: stamp(),
            flowed_back: merged_ids(&accounting.merges),
            foreign_commits: accounting.foreign(),
            merges: accounting.merges.clone(),
        });
        queue.last_sync = Some(LastSync {
            at: stamp(),
            upstream_sha: from_sha.clone().unwrap_or_else(|| sha.clone()),
            result: "pending-approval".into(),
            message: Some(format!(
                "Waiting for from-upstream approval of {}",
                accounting.pending_sha
            )),
        });
        write_queue_file(repo, &queue)?;
        commit_queue(repo, "uplink: from-upstream packet")?;
        Ok(SyncResult {
            queue,
            needs_approval: true,
            pending_sha: Some(accounting.pending_sha.clone()),
            flowed_back: merged_ids(&accounting.merges),
            foreign_commits: accounting.foreign(),
            claims: accounting.claims(),
            merges: accounting.merges,
            report_path: Some(report_path),
            report: Some(report),
        })
    })
}

pub fn accept_upstream(repo: &Path) -> Result<SyncResult> {
    accept_upstream_at(repo, None)
}

/// Promotes the pending upstream. With `expected_sha`, refuses when the
/// pending upstream is not the one that was reviewed.
pub fn accept_upstream_at(repo: &Path, expected_sha: Option<&str>) -> Result<SyncResult> {
    with_queue_lock(repo, || {
        let queue = read_queue_file(repo)?;
        let pending = queue.pending_upstream.clone().ok_or_else(|| {
            Error::msg("No pending upstream to accept. Run `git uplink sync` first.")
        })?;
        if let Some(expected) = expected_sha
            && expected != pending.sha
        {
            return Err(Error::msg(format!(
                "Pending upstream is {}, not the reviewed {expected}; review the new packet and approve again.",
                pending.sha
            )));
        }
        fetch_upstream_remote(repo, &queue)?;
        if !has_ref(repo, &pending.sha)? {
            return Err(Error::msg(format!(
                "Pending upstream {} is not available; fetch public main and try again.",
                pending.sha
            )));
        }
        // A packet written before `merges` was recorded only lists ids.
        let merges = if pending.merges.is_empty() && !pending.flowed_back.is_empty() {
            account_incoming(
                repo,
                &queue,
                pending.from_sha.as_deref(),
                &pending.sha,
                &SyncOpts::default(),
            )?
            .merges
        } else {
            pending.merges.clone()
        };
        let queue = apply_fetched_upstream(repo, &pending.sha, &merges)?;
        Ok(SyncResult::applied_with(queue, merges))
    })
}
