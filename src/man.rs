//! The man pages `build.rs` generates from the clap definitions.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

include!(concat!(env!("OUT_DIR"), "/man_pages.rs"));

/// Writes every embedded page into `dir`, creating it, and returns the paths.
pub fn write_man_pages(dir: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)
        .map_err(|err| Error::msg(format!("could not create {}: {err}", dir.display())))?;
    MAN_PAGES
        .iter()
        .map(|(name, page)| {
            let path = dir.join(name);
            fs::write(&path, page)
                .map_err(|err| Error::msg(format!("could not write {}: {err}", path.display())))?;
            Ok(path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
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
}
