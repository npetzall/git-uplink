//! The web UI and forge packs `build.rs` embeds, as tables of `/`-separated
//! relative paths and bytes, sorted by path.

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// The bytes of `path` in `files`.
pub fn get(files: &'static [(&str, &[u8])], path: &str) -> Option<&'static [u8]> {
    files
        .binary_search_by(|(name, _)| (*name).cmp(path))
        .ok()
        .map(|index| files[index].1)
}

#[cfg(test)]
#[path = "embedded_test.rs"]
mod tests;
