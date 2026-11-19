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
#[path = "man_test.rs"]
mod tests;
