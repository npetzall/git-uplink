//! The groups the adopt UI builds, as a tree over first-parent history:
//! each group is a contiguous run of commits, and a commit in no group is
//! skipped. Commits keep their order; nothing here can reorder them.

use std::collections::BTreeMap;

use crate::adopt::{AdoptCommit, AdoptGroup};
use crate::assess::strip_html_comments;
use crate::edit::TextEdit;
use crate::types::{PatchIntent, first_line};

pub type GroupId = usize;

/// What a group says about its patch, and what the run does with it.
pub struct GroupForm {
    pub upstream: bool,
    pub message: TextEdit,
    /// Adopt it even when `preflight.sh` fails (`ignorePreflight`).
    pub skip_preflight: bool,
    /// Pause on it to amend before it is checked (`stopToAmend`).
    pub stop_to_amend: bool,
    /// The amended commit from an earlier run, with the commits it was
    /// made from. It only holds while the group has exactly those.
    pub amended: Option<(String, Vec<usize>)>,
}

impl GroupForm {
    /// The first line of the message, to name the group.
    pub fn subject(&self) -> &str {
        first_line(self.message.text())
    }
}

/// One row of the tree, in history order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Group(GroupId),
    /// A commit in a group, shown under it.
    Member(usize),
    Skipped(usize),
}

impl Row {
    pub fn commit(self) -> Option<usize> {
        match self {
            Row::Member(i) | Row::Skipped(i) => Some(i),
            Row::Group(_) => None,
        }
    }
}

/// What can be done with one commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewGroup,
    /// Move to (or, for a skipped commit, add to) the group above.
    ToPrevious,
    /// Move to (or, for a skipped commit, add to) the group below.
    ToNext,
    Skip,
}

pub struct AdoptTree {
    commits: Vec<AdoptCommit>,
    owner: Vec<Option<GroupId>>,
    groups: BTreeMap<GroupId, GroupForm>,
    next_id: GroupId,
    allow_skip: bool,
    marker: String,
}

impl AdoptTree {
    /// Each commit in a group of its own, except that, with `allow_skip`,
    /// a commit already on main starts out skipped.
    pub fn new(commits: Vec<AdoptCommit>, allow_skip: bool, marker: &str) -> Self {
        let mut tree = Self {
            owner: vec![None; commits.len()],
            commits,
            groups: BTreeMap::new(),
            next_id: 0,
            allow_skip,
            marker: marker.to_string(),
        };
        for i in 0..tree.commits.len() {
            if !(allow_skip && tree.commits[i].on_main) {
                let id = tree.new_group_from(i);
                tree.owner[i] = Some(id);
            }
        }
        tree
    }

    /// The tree of `groups`, which [`crate::adopt::resolve_groups`] has
    /// checked against `commits`: full shas, in order, none left out unless
    /// `allow_skip`.
    pub fn from_groups(
        commits: Vec<AdoptCommit>,
        allow_skip: bool,
        marker: &str,
        groups: &[AdoptGroup],
    ) -> Result<Self, String> {
        let mut tree = Self {
            owner: vec![None; commits.len()],
            commits,
            groups: BTreeMap::new(),
            next_id: 0,
            allow_skip,
            marker: marker.to_string(),
        };
        for group in groups {
            let id = tree.next_id;
            tree.next_id += 1;
            let mut members = Vec::new();
            for sha in &group.commits {
                let i = tree
                    .commits
                    .iter()
                    .position(|c| &c.sha == sha)
                    .ok_or_else(|| format!("commit {sha} is not one to adopt"))?;
                tree.owner[i] = Some(id);
                members.push(i);
            }
            let mut message = group.message.clone();
            if !strip_html_comments(&message).contains(&tree.marker) {
                message.push_str("\n\n");
                message.push_str(&tree.marker);
                message.push('\n');
            }
            tree.groups.insert(
                id,
                GroupForm {
                    upstream: !group.intent.is_internal_only(),
                    message: TextEdit::multiline(message),
                    skip_preflight: group.ignore_preflight,
                    stop_to_amend: group.stop_to_amend,
                    amended: group.amended.clone().map(|sha| (sha, members)),
                },
            );
        }
        if !allow_skip && tree.owner.iter().any(Option::is_none) {
            return Err("init adopts every commit; each must be in a group".into());
        }
        Ok(tree)
    }

    /// Groups whose amended commit still holds.
    pub fn amended_count(&self) -> usize {
        self.group_ids()
            .into_iter()
            .filter(|id| self.amended_of(*id).is_some())
            .count()
    }

    /// The amended commit of group `id`, while its members are unchanged.
    pub fn amended_of(&self, id: GroupId) -> Option<&str> {
        let (sha, members) = self.groups[&id].amended.as_ref()?;
        (*members == self.members(id)).then_some(sha.as_str())
    }

    pub fn commits(&self) -> &[AdoptCommit] {
        &self.commits
    }

    pub fn allow_skip(&self) -> bool {
        self.allow_skip
    }

    pub fn owner(&self, commit: usize) -> Option<GroupId> {
        self.owner[commit]
    }

    pub fn form(&self, id: GroupId) -> &GroupForm {
        &self.groups[&id]
    }

    pub fn form_mut(&mut self, id: GroupId) -> &mut GroupForm {
        self.groups.get_mut(&id).expect("group exists")
    }

    /// Groups in history order.
    pub fn group_ids(&self) -> Vec<GroupId> {
        let mut ids = Vec::new();
        for id in self.owner.iter().flatten() {
            if ids.last() != Some(id) {
                ids.push(*id);
            }
        }
        ids
    }

    /// The group's number in history order, from 1.
    pub fn number(&self, id: GroupId) -> usize {
        self.group_ids()
            .iter()
            .position(|g| *g == id)
            .map_or(0, |i| i + 1)
    }

    /// The group just above `id` in history, when it is internal-only and
    /// `id` is upstream-bound: the patch cannot depend on it.
    pub fn after_internal_only(&self, id: GroupId) -> bool {
        let ids = self.group_ids();
        let Some(pos) = ids.iter().position(|g| *g == id) else {
            return false;
        };
        pos > 0 && self.groups[&id].upstream && !self.groups[&ids[pos - 1]].upstream
    }

    pub fn members(&self, id: GroupId) -> Vec<usize> {
        (0..self.owner.len())
            .filter(|i| self.owner[*i] == Some(id))
            .collect()
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, owner) in self.owner.iter().enumerate() {
            match owner {
                Some(id) => {
                    if i == 0 || self.owner[i - 1] != Some(*id) {
                        rows.push(Row::Group(*id));
                    }
                    rows.push(Row::Member(i));
                }
                None => rows.push(Row::Skipped(i)),
            }
        }
        rows
    }

    /// Groups, commits in a group, and skipped commits.
    pub fn counts(&self) -> (usize, usize, usize) {
        let grouped = self.owner.iter().filter(|o| o.is_some()).count();
        (self.group_ids().len(), grouped, self.owner.len() - grouped)
    }

    /// Every action for `commit`, and whether it can be taken.
    pub fn actions(&self, commit: usize) -> Vec<(Action, bool)> {
        let above = commit.checked_sub(1).and_then(|i| self.owner[i]);
        let below = self.owner.get(commit + 1).copied().flatten();
        match self.owner[commit] {
            Some(id) => {
                let members = self.members(id);
                let (first, last) = (members[0], *members.last().unwrap());
                let prev = first.checked_sub(1).and_then(|i| self.owner[i]);
                let next = self.owner.get(last + 1).copied().flatten();
                vec![
                    (Action::NewGroup, members.len() > 1),
                    (Action::ToPrevious, prev.is_some()),
                    (Action::ToNext, next.is_some()),
                    (Action::Skip, self.allow_skip),
                ]
            }
            None => vec![
                (Action::NewGroup, true),
                (Action::ToPrevious, above.is_some()),
                (Action::ToNext, below.is_some()),
                (Action::Skip, false),
            ],
        }
    }

    /// Takes `action` on `commit`. Returns the group it is in afterwards.
    pub fn apply(
        &mut self,
        commit: usize,
        action: Action,
    ) -> Result<Option<GroupId>, &'static str> {
        let enabled = self
            .actions(commit)
            .into_iter()
            .any(|(a, ok)| a == action && ok);
        if !enabled {
            return Err(match action {
                Action::Skip if !self.allow_skip => "init adopts every commit; it cannot skip one",
                Action::Skip => "the commit is skipped already",
                Action::NewGroup => "the commit is the only one in its group already",
                Action::ToPrevious => "there is no group directly above",
                Action::ToNext => "there is no group directly below",
            });
        }
        match (self.owner[commit], action) {
            (None, Action::NewGroup) => {
                let id = self.new_group_from(commit);
                self.owner[commit] = Some(id);
            }
            (None, Action::ToPrevious) => self.owner[commit] = self.owner[commit - 1],
            (None, Action::ToNext) => self.owner[commit] = self.owner[commit + 1],
            (Some(_), Action::NewGroup) => {
                let id = self.new_group_from(commit);
                self.split_out(commit, Some(id));
            }
            (Some(_), Action::Skip) => self.split_out(commit, None),
            (Some(id), Action::ToPrevious) => {
                let members = self.members(id);
                let prev = self.owner[members[0] - 1];
                for i in members[0]..=commit {
                    self.owner[i] = prev;
                }
            }
            (Some(id), Action::ToNext) => {
                let members = self.members(id);
                let last = *members.last().unwrap();
                let next = self.owner[last + 1];
                for i in commit..=last {
                    self.owner[i] = next;
                }
            }
            (None, Action::Skip) => {}
        }
        self.drop_empty();
        Ok(self.owner[commit])
    }

    /// The groups to adopt, in history order.
    pub fn to_groups(&self) -> Vec<AdoptGroup> {
        self.group_ids()
            .into_iter()
            .map(|id| {
                let form = &self.groups[&id];
                AdoptGroup {
                    commits: self
                        .members(id)
                        .into_iter()
                        .map(|i| self.commits[i].sha.clone())
                        .collect(),
                    intent: if form.upstream {
                        PatchIntent::Upstream
                    } else {
                        PatchIntent::InternalOnly
                    },
                    message: self.message_of(form.message.text()),
                    ignore_preflight: form.skip_preflight,
                    stop_to_amend: form.stop_to_amend,
                    amended: self.amended_of(id).map(str::to_string),
                }
            })
            .collect()
    }

    /// `text`, without the cutoff marker when nothing follows it.
    fn message_of(&self, text: &str) -> String {
        match text.find(&self.marker) {
            Some(at) if text[at + self.marker.len()..].trim().is_empty() => {
                text[..at].trim_end().to_string()
            }
            _ => text.trim_end().to_string(),
        }
    }

    /// Takes `commit` out of its group, into `to`. Commits above it stay in
    /// the group; commits below it move to a new group, or keep the group
    /// when none are above.
    fn split_out(&mut self, commit: usize, to: Option<GroupId>) {
        let id = self.owner[commit].expect("commit is in a group");
        let members = self.members(id);
        let has_above = members[0] < commit;
        let below: Vec<usize> = members.iter().copied().filter(|i| *i > commit).collect();
        self.owner[commit] = to;
        if has_above && !below.is_empty() {
            let rest = self.new_group_from(below[0]);
            for i in below {
                self.owner[i] = Some(rest);
            }
        }
    }

    fn new_group_from(&mut self, commit: usize) -> GroupId {
        let id = self.next_id;
        self.next_id += 1;
        let c = &self.commits[commit];
        let mut message = c.subject.clone();
        if !c.body.is_empty() {
            message.push_str("\n\n");
            message.push_str(&c.body);
        }
        // A body that already has the cutoff keeps it; it is not added twice.
        if !strip_html_comments(&message).contains(&self.marker) {
            message.push_str("\n\n");
            message.push_str(&self.marker);
            message.push('\n');
        }
        self.groups.insert(
            id,
            GroupForm {
                upstream: true,
                message: TextEdit::multiline(message),
                skip_preflight: false,
                stop_to_amend: false,
                amended: None,
            },
        );
        id
    }

    fn drop_empty(&mut self) {
        let used = self.group_ids();
        self.groups.retain(|id, _| used.contains(id));
    }
}

#[cfg(test)]
mod tests {
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
}
