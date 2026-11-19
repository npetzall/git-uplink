use super::*;

const MARKER: &str = "----- Uplink: internal below this line -----";

fn commit(n: usize, on_main: bool) -> AdoptCommit {
    AdoptCommit {
        sha: format!("{n:040}"),
        short: format!("{n:07}"),
        subject: format!("subject {n}"),
        body: if n == 1 {
            "body 1".into()
        } else {
            String::new()
        },
        is_merge: false,
        parent_count: 1,
        first_parent: format!("{:040}", n + 100),
        on_main,
    }
}

fn tree(n: usize, allow_skip: bool) -> AdoptTree {
    AdoptTree::new(
        (0..n).map(|i| commit(i, false)).collect(),
        allow_skip,
        MARKER,
    )
}

/// Group number per commit, 0 for skipped, after renumbering in order.
fn shape(tree: &AdoptTree) -> Vec<usize> {
    (0..tree.commits().len())
        .map(|i| tree.owner(i).map_or(0, |id| tree.number(id)))
        .collect()
}

fn contiguous(tree: &AdoptTree) -> bool {
    tree.group_ids()
        .iter()
        .all(|id| tree.members(*id).windows(2).all(|w| w[1] == w[0] + 1))
}

#[test]
fn starts_with_a_group_per_commit_and_on_main_skipped() {
    let commits = vec![commit(0, true), commit(1, false), commit(2, false)];
    let t = AdoptTree::new(commits.clone(), true, MARKER);
    assert_eq!(shape(&t), [0, 1, 2]);
    assert_eq!(
        t.rows(),
        [
            Row::Skipped(0),
            Row::Group(0),
            Row::Member(1),
            Row::Group(1),
            Row::Member(2)
        ]
    );
    // init cannot skip, so on main means nothing there.
    assert_eq!(shape(&AdoptTree::new(commits, false, MARKER)), [1, 2, 3]);
}

#[test]
fn new_groups_are_filled_from_the_commit() {
    let t = tree(2, true);
    let form = t.form(t.owner(1).unwrap());
    assert_eq!(form.subject(), "subject 1");
    assert!(form.upstream);
    assert_eq!(
        form.message.text(),
        format!("subject 1\n\nbody 1\n\n{MARKER}\n")
    );
    // The empty internal part is left out.
    assert_eq!(t.to_groups()[1].message, "subject 1\n\nbody 1");
}

#[test]
fn a_body_with_the_cutoff_keeps_one_cutoff() {
    let mut c = commit(0, false);
    c.body = format!("Public.\n\n{MARKER}\nTicket: PROJ-1");
    let t = AdoptTree::new(vec![c], false, MARKER);
    let form = t.form(t.owner(0).unwrap());
    assert_eq!(
        form.message.text(),
        format!("subject 0\n\nPublic.\n\n{MARKER}\nTicket: PROJ-1")
    );
    assert_eq!(
        t.to_groups()[0].message,
        format!("subject 0\n\nPublic.\n\n{MARKER}\nTicket: PROJ-1")
    );
}

#[test]
fn a_cutoff_in_an_html_comment_does_not_count() {
    let mut c = commit(0, false);
    c.body = format!("<!-- {MARKER} -->");
    let t = AdoptTree::new(vec![c], false, MARKER);
    let text = t.form(t.owner(0).unwrap()).message.text().to_string();
    assert!(text.ends_with(&format!("-->\n\n{MARKER}\n")), "{text}");
}

#[test]
fn skipped_commits_join_neighbours_only_when_adjacent() {
    let mut t = tree(4, true);
    t.apply(1, Action::Skip).unwrap();
    t.apply(2, Action::Skip).unwrap();
    assert_eq!(shape(&t), [1, 0, 0, 2]);
    assert!(t.apply(2, Action::ToPrevious).is_err());
    t.apply(1, Action::ToPrevious).unwrap();
    t.apply(2, Action::ToNext).unwrap();
    assert_eq!(shape(&t), [1, 1, 2, 2]);
    assert!(t.apply(1, Action::Skip).is_ok());
    assert!(t.apply(1, Action::Skip).is_err());
}

#[test]
fn splitting_the_middle_of_a_group_keeps_order() {
    let mut t = tree(5, true);
    for i in 1..5 {
        t.apply(i, Action::ToPrevious).unwrap();
    }
    assert_eq!(shape(&t), [1, 1, 1, 1, 1]);
    let first = t.owner(0).unwrap();
    t.form_mut(first).message = TextEdit::multiline("kept\n\nbody");

    t.apply(2, Action::NewGroup).unwrap();
    assert_eq!(shape(&t), [1, 1, 2, 3, 3]);
    assert_eq!(t.form(t.owner(0).unwrap()).subject(), "kept");
    assert_eq!(t.form(t.owner(3).unwrap()).subject(), "subject 3");

    t.apply(3, Action::Skip).unwrap();
    assert_eq!(shape(&t), [1, 1, 2, 0, 3]);
    assert!(contiguous(&t));
}

#[test]
fn splitting_the_top_of_a_group_keeps_the_group_below() {
    let mut t = tree(3, true);
    t.apply(1, Action::ToPrevious).unwrap();
    t.apply(2, Action::ToPrevious).unwrap();
    let id = t.owner(0).unwrap();
    t.apply(0, Action::Skip).unwrap();
    assert_eq!(shape(&t), [0, 1, 1]);
    assert_eq!(t.owner(1), Some(id));
}

#[test]
fn moving_takes_the_commits_between_along() {
    let mut t = tree(4, true);
    t.apply(2, Action::ToPrevious).unwrap();
    t.apply(3, Action::ToPrevious).unwrap();
    assert_eq!(shape(&t), [1, 2, 2, 2]);
    // 1..=2 of group 2 go up into group 1.
    t.apply(2, Action::ToPrevious).unwrap();
    assert_eq!(shape(&t), [1, 1, 1, 2]);
    // 1..=2 go down into the group below.
    t.apply(1, Action::ToNext).unwrap();
    assert_eq!(shape(&t), [1, 2, 2, 2]);
    assert!(contiguous(&t));
    assert_eq!(t.counts(), (2, 4, 0));
}

#[test]
fn disabled_actions_say_why() {
    let mut t = tree(2, false);
    assert_eq!(
        t.apply(0, Action::Skip),
        Err("init adopts every commit; it cannot skip one")
    );
    assert!(t.apply(0, Action::NewGroup).is_err());
    assert!(t.apply(0, Action::ToPrevious).is_err());
    let actions = t.actions(0);
    assert_eq!(
        actions,
        [
            (Action::NewGroup, false),
            (Action::ToPrevious, false),
            (Action::ToNext, true),
            (Action::Skip, false)
        ]
    );
}

#[test]
fn to_groups_lists_full_shas_in_order_with_intent() {
    let mut t = tree(3, true);
    t.apply(1, Action::ToPrevious).unwrap();
    let last = t.owner(2).unwrap();
    t.form_mut(last).upstream = false;
    let groups = t.to_groups();
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups[0].commits,
        [commit(0, false).sha, commit(1, false).sha]
    );
    assert_eq!(groups[0].intent, PatchIntent::Upstream);
    assert_eq!(groups[1].intent, PatchIntent::InternalOnly);
    assert!(!t.after_internal_only(last));
}

#[test]
fn from_groups_round_trips_the_run_decisions() {
    let commits: Vec<AdoptCommit> = (0..3).map(|i| commit(i, false)).collect();
    let groups = vec![
        AdoptGroup {
            commits: vec![commits[0].sha.clone(), commits[1].sha.clone()],
            message: "First".into(),
            intent: PatchIntent::Upstream,
            ignore_preflight: true,
            amended: Some("a".repeat(40)),
            stop_to_amend: false,
        },
        AdoptGroup {
            commits: vec![commits[2].sha.clone()],
            message: "Second".into(),
            intent: PatchIntent::InternalOnly,
            stop_to_amend: true,
            ..Default::default()
        },
    ];
    let t = AdoptTree::from_groups(commits, true, MARKER, &groups).unwrap();
    assert_eq!(t.to_groups(), groups);
}

#[test]
fn an_amended_mark_is_dropped_when_the_group_changes() {
    let commits: Vec<AdoptCommit> = (0..3).map(|i| commit(i, false)).collect();
    let groups = vec![AdoptGroup {
        commits: vec![commits[0].sha.clone(), commits[1].sha.clone()],
        message: "First".into(),
        amended: Some("a".repeat(40)),
        ..Default::default()
    }];
    let mut t = AdoptTree::from_groups(commits, true, MARKER, &groups).unwrap();
    assert_eq!(t.amended_count(), 1);
    t.apply(2, Action::ToPrevious).unwrap();
    assert_eq!(t.amended_count(), 0);
    assert_eq!(t.to_groups()[0].amended, None);
}

#[test]
fn from_groups_needs_every_commit_when_none_may_be_skipped() {
    let commits: Vec<AdoptCommit> = (0..2).map(|i| commit(i, false)).collect();
    let groups = vec![AdoptGroup {
        commits: vec![commits[0].sha.clone()],
        message: "First".into(),
        ..Default::default()
    }];
    assert!(AdoptTree::from_groups(commits, false, MARKER, &groups).is_err());
}
