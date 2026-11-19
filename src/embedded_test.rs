use super::{PACK_GITHUB, PACK_GITHUB_HOOKS, PACK_TRY_IT_ON_GITHUB, WEB, get};

#[test]
fn tables_are_sorted_and_use_forward_slashes() {
    for table in [WEB, PACK_GITHUB, PACK_GITHUB_HOOKS, PACK_TRY_IT_ON_GITHUB] {
        assert!(!table.is_empty());
        assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(table.iter().all(|(path, _)| !path.contains('\\')));
    }
}

#[test]
fn packs_include_dotfiles() {
    assert!(
        PACK_GITHUB
            .iter()
            .any(|(path, _)| path.starts_with(".github/"))
    );
    assert!(
        PACK_GITHUB_HOOKS
            .iter()
            .any(|(path, _)| path.starts_with(".github/"))
    );
}

#[test]
fn get_finds_present_paths_only() {
    assert!(get(WEB, "index.html").is_some_and(|bytes| !bytes.is_empty()));
    assert!(get(WEB, "missing.html").is_none());
}
