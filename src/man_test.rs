use super::*;

#[test]
fn writes_a_page_for_the_command_and_each_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("man1");
    let written = write_man_pages(&target).unwrap();
    assert_eq!(written.len(), MAN_PAGES.len());
    for name in ["git-uplink.1", "git-uplink-init.1", "git-uplink-man.1"] {
        let page = fs::read_to_string(target.join(name)).unwrap();
        assert!(page.starts_with(".ie"), "{name} is not roff: {page:.40}");
    }
}
