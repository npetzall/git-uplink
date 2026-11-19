#[test]
fn web_ui_index_is_embedded() {
    assert!(
        crate::webui::has_embedded_index(),
        "web/dist/index.html must be produced by build.rs and embedded"
    );
}
