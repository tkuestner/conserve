// Copyright 2015, 2016, 2017, 2018, 2019, 2020 Martin Pool.

// This program is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation; either version 2 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Restore from the archive to the filesystem.

use std::fs;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use filetime::{set_file_handle_times, set_symlink_file_times};
use globset::GlobSet;

use crate::copy_tree::copy_tree;
use crate::entry::Entry;
use crate::io::{directory_is_empty, ensure_dir_exists};
use crate::stats::CopyStats;
use crate::unix_time::UnixTime;
use crate::*;
use crate::{band::BandSelectionPolicy, copy_tree::CopyOptions};

/// Description of how to restore a tree.
#[derive(Debug)]
pub struct RestoreOptions {
    pub print_filenames: bool,
    pub excludes: Option<GlobSet>,
    /// Restore only this subdirectory.
    pub only_subtree: Option<Apath>,
    pub overwrite: bool,
    // The band to select, or by default the last complete one.
    pub band_selection: BandSelectionPolicy,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        RestoreOptions {
            print_filenames: false,
            overwrite: false,
            band_selection: BandSelectionPolicy::LatestClosed,
            excludes: None,
            only_subtree: None,
        }
    }
}

/// Restore a selected version, or by default the latest, to a destination directory.
pub fn restore(
    archive: &Archive,
    destination_path: &Path,
    options: &RestoreOptions,
) -> Result<CopyStats> {
    let st = archive.open_stored_tree(options.band_selection.clone())?;
    let rt = if options.overwrite {
        RestoreTree::create_overwrite(destination_path)
    } else {
        RestoreTree::create(destination_path)
    }?;
    let opts = CopyOptions {
        print_filenames: options.print_filenames,
        only_subtree: options.only_subtree.clone(),
        excludes: options.excludes.clone(),
        ..CopyOptions::default()
    };
    copy_tree(&st, rt, &opts)
}

/// A write-only tree on the filesystem, as a restore destination.
#[derive(Debug)]
pub struct RestoreTree {
    path: PathBuf,

    dir_mtimes: Vec<(PathBuf, UnixTime)>,
}

impl RestoreTree {
    fn new(path: PathBuf) -> RestoreTree {
        RestoreTree {
            path,
            dir_mtimes: Vec::new(),
        }
    }

    /// Create a RestoreTree.
    ///
    /// The destination must either not yet exist, or be an empty directory.
    pub fn create<P: Into<PathBuf>>(path: P) -> Result<RestoreTree> {
        let path = path.into();
        match ensure_dir_exists(&path).and_then(|()| directory_is_empty(&path)) {
            Err(source) => Err(Error::Restore { path, source }),
            Ok(true) => Ok(RestoreTree::new(path)),
            Ok(false) => Err(Error::DestinationNotEmpty { path }),
        }
    }

    /// Create a RestoreTree, even if the destination directory is not empty.
    pub fn create_overwrite(path: &Path) -> Result<RestoreTree> {
        Ok(RestoreTree::new(path.to_path_buf()))
    }

    fn rooted_path(&self, apath: &Apath) -> PathBuf {
        // Remove initial slash so that the apath is relative to the destination.
        self.path.join(&apath[1..])
    }
}

impl tree::WriteTree for RestoreTree {
    fn finish(self) -> Result<CopyStats> {
        for (path, time) in self.dir_mtimes {
            if let Err(err) = filetime::set_file_mtime(path, time.into()) {
                ui::problem(&format!("Failed to set directory mtime: {:?}", err));
            }
        }
        Ok(CopyStats::default())
    }

    fn copy_dir<E: Entry>(&mut self, entry: &E) -> Result<()> {
        let path = self.rooted_path(entry.apath());
        if let Err(source) = fs::create_dir_all(&path) {
            if source.kind() != io::ErrorKind::AlreadyExists {
                return Err(Error::Restore { path, source });
            }
        }
        self.dir_mtimes.push((path, entry.mtime()));
        Ok(())
    }

    /// Copy in the contents of a file from another tree.
    fn copy_file<R: ReadTree>(
        &mut self,
        source_entry: &R::Entry,
        from_tree: &R,
    ) -> Result<CopyStats> {
        // TODO: Restore permissions.
        let path = self.rooted_path(source_entry.apath());
        let restore_err = |source| Error::Restore {
            path: path.clone(),
            source,
        };
        let mut restore_file = File::create(&path).map_err(restore_err)?;
        // TODO: Read one block at a time: don't pull all the contents into memory.
        let content = &mut from_tree.file_contents(&source_entry)?;
        let bytes_copied = std::io::copy(content, &mut restore_file).map_err(restore_err)?;
        restore_file.flush().map_err(restore_err)?;

        let mtime = Some(source_entry.mtime().into());
        set_file_handle_times(&restore_file, mtime, mtime).map_err(|source| {
            Error::RestoreModificationTime {
                path: path.clone(),
                source,
            }
        })?;

        // TODO: Accumulate more stats.
        Ok(CopyStats {
            uncompressed_bytes: bytes_copied,
            ..CopyStats::default()
        })
    }

    #[cfg(unix)]
    fn copy_symlink<E: Entry>(&mut self, entry: &E) -> Result<()> {
        use std::os::unix::fs as unix_fs;
        if let Some(ref target) = entry.symlink_target() {
            let path = self.rooted_path(entry.apath());
            if let Err(source) = unix_fs::symlink(target, &path) {
                return Err(Error::Restore { path, source });
            }
            let mtime = entry.mtime().into();
            if let Err(source) = set_symlink_file_times(&path, mtime, mtime) {
                return Err(Error::RestoreModificationTime { path, source });
            }
        } else {
            // TODO: Treat as an error.
            ui::problem(&format!("No target in symlink entry {}", entry.apath()));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn copy_symlink<E: Entry>(&mut self, entry: &E) -> Result<()> {
        // TODO: Add a test with a canned index containing a symlink, and expect
        // it cannot be restored on Windows and can be on Unix.
        ui::problem(&format!(
            "Can't restore symlinks on non-Unix: {}",
            entry.apath()
        ));
        Ok(())
    }
}
