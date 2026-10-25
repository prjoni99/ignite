//! Continuous mirror of Amazon's stock notebooks into Ignite: ledger, source stamp, scan.
//!
//! This is the pure, no-decoding half. A [`MirrorLedger`] remembers, per Amazon notebook, the
//! native copy built from it and the [`SourceStamp`] it was built from. [`scan`] compares the
//! ledger with what Amazon has on disk using file metadata only (it never opens `nbk` or
//! `actions.log` for reading) and classifies every notebook into a [`MirrorAction`].
//!
//! Amazon state is strictly read-only: the notebooks root and the Kindle catalog are never
//! written. The ledger lives in the Scribe library at `<library>/.scribe-mirror/ledger.json`
//! and, like the import ledger, fails closed when it cannot be trusted.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::nbk_import::{discover, write_atomic, ImportSources, SourceNotebook};

// ------------------------------------------------------------------------------ constants

/// Ledger directory inside the Scribe library.
pub const MIRROR_DIR: &str = ".scribe-mirror";
pub const LEDGER_FILE: &str = "ledger.json";
/// Provenance marker written into each mirrored notebook.
pub const MIRROR_MARKER: &str = "mirror-origin.json";
pub const AMAZON_FOLDER_TITLE: &str = "Amazon";
pub const REMOVED_FOLDER_TITLE: &str = "Removed from Amazon";
pub const ORIGIN_KIND: &str = "amazon_nbk";

// ------------------------------------------------------------------------------ stamp

/// Cheap change detector for one Amazon notebook: length and mtime (ms since the Unix epoch)
/// of `nbk` and `actions.log`. The title is deliberately not part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStamp {
    pub nbk_len: u64,
    pub nbk_mtime_ms: u64,
    pub actions_len: u64,
    pub actions_mtime_ms: u64,
}

fn file_stamp(path: &Path, required: bool) -> std::io::Result<(u64, u64)> {
    match fs::metadata(path) {
        Ok(m) => {
            let ms = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            Ok((m.len(), ms))
        }
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => Ok((0, 0)),
        Err(e) => Err(e),
    }
}

fn try_stamp(dir: &Path) -> std::io::Result<SourceStamp> {
    let (nbk_len, nbk_mtime_ms) = file_stamp(&dir.join("nbk"), true)?;
    let (actions_len, actions_mtime_ms) = file_stamp(&dir.join("actions.log"), false)?;
    Ok(SourceStamp {
        nbk_len,
        nbk_mtime_ms,
        actions_len,
        actions_mtime_ms,
    })
}

/// Stamp a source notebook directory. Metadata only; an absent `actions.log` stamps as zeros.
pub fn stamp_source(dir: &Path) -> Result<SourceStamp, String> {
    try_stamp(dir).map_err(|e| format!("stat {}: {e}", dir.display()))
}

// ------------------------------------------------------------------------------ ledger

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorState {
    Current,
    Rebuilding,
    Removed,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorEntryV1 {
    pub origin_id: String,
    pub title: String,
    pub state: MirrorState,
    /// `None` until a notebook exists (a create that failed before building anything).
    pub dest_notebook_id: Option<String>,
    pub stamp: Option<SourceStamp>,
    pub content_sha256: Option<String>,
    pub pages: usize,
    pub error: Option<String>,
    pub updated_unix_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorLedgerV1 {
    pub v: u32,
    /// Key = `origin_id`.
    pub entries: BTreeMap<String, MirrorEntryV1>,
    /// Local folder ids.
    pub amazon_folder: Option<String>,
    pub removed_folder: Option<String>,
    pub last_scan_unix_ms: Option<u64>,
    pub last_scan_error: Option<String>,
}

/// The mirror ledger. A corrupt ledger fails closed (what was mirrored cannot be proven).
#[derive(Debug)]
pub struct MirrorLedger {
    path: PathBuf,
    pub data: MirrorLedgerV1,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl MirrorLedger {
    pub fn path_for(library_root: &Path) -> PathBuf {
        library_root.join(MIRROR_DIR).join(LEDGER_FILE)
    }

    /// A missing ledger is empty; an unreadable one or any version other than 1 is refused.
    pub fn open(library_root: &Path) -> Result<Self, String> {
        let path = Self::path_for(library_root);
        let data = match fs::read(&path) {
            Ok(b) => {
                let d: MirrorLedgerV1 = serde_json::from_slice(&b)
                    .map_err(|e| format!("REFUSE MIRROR: unreadable mirror ledger: {e}"))?;
                if d.v != 1 {
                    return Err(format!("REFUSE MIRROR: mirror ledger version {}", d.v));
                }
                d
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => MirrorLedgerV1 {
                v: 1,
                ..Default::default()
            },
            Err(e) => return Err(format!("REFUSE MIRROR: read mirror ledger: {e}")),
        };
        Ok(Self { path, data })
    }

    /// Apply `f`, persist atomically, and restore the previous in-memory state if the write
    /// fails (memory == disk).
    fn commit(&mut self, f: impl FnOnce(&mut MirrorLedgerV1)) -> Result<(), String> {
        let prev = self.data.clone();
        f(&mut self.data);
        let res = serde_json::to_vec_pretty(&self.data)
            .map_err(|e| e.to_string())
            .and_then(|bytes| write_atomic(&self.path, &bytes));
        if let Err(e) = res {
            self.data = prev;
            return Err(format!("mirror ledger write: {e}"));
        }
        Ok(())
    }

    pub fn put(&mut self, e: MirrorEntryV1) -> Result<(), String> {
        self.commit(|d| {
            d.entries.insert(e.origin_id.clone(), e);
        })
    }

    /// Drop an entry (the engine does this for a create that never built anything).
    pub fn remove(&mut self, origin_id: &str) -> Result<(), String> {
        self.commit(|d| {
            d.entries.remove(origin_id);
        })
    }

    pub fn set_folders(
        &mut self,
        amazon: Option<String>,
        removed: Option<String>,
    ) -> Result<(), String> {
        self.commit(|d| {
            d.amazon_folder = amazon;
            d.removed_folder = removed;
        })
    }

    /// Record a scan attempt now, with its error if it failed.
    pub fn note_scan(&mut self, error: Option<String>) -> Result<(), String> {
        self.commit(|d| {
            d.last_scan_unix_ms = Some(now_ms());
            d.last_scan_error = error;
        })
    }
}

// ------------------------------------------------------------------------------ scan

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorAction {
    /// No ledger entry yet.
    Create {
        source: SourceNotebook,
        stamp: SourceStamp,
    },
    /// Stamp differs from the entry (or a rebuild was interrupted).
    Rebuild {
        source: SourceNotebook,
        stamp: SourceStamp,
    },
    /// Same stamp, title differs.
    Rename { source: SourceNotebook },
    /// Entry was `Removed` and the source is back.
    Restore {
        source: SourceNotebook,
        stamp: SourceStamp,
    },
    /// Entry still live (or never built) and the source is gone.
    Remove { origin_id: String },
}

impl MirrorAction {
    fn origin_id(&self) -> &str {
        match self {
            Self::Create { source, .. }
            | Self::Rebuild { source, .. }
            | Self::Rename { source }
            | Self::Restore { source, .. } => &source.origin_id,
            Self::Remove { origin_id } => origin_id,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorScan {
    /// Sorted by origin id.
    pub actions: Vec<MirrorAction>,
    /// Notebooks already in step with Amazon.
    pub current: usize,
    /// `(title, error)` of sources skipped because their stamp has not changed since they
    /// were found unsupported.
    pub unsupported: Vec<(String, String)>,
}

/// Classify every Amazon notebook against the ledger. Metadata only, never decodes.
/// `discover`'s error is returned as-is; a missing catalog or notebooks root is just empty.
pub fn scan(src: &ImportSources, ledger: &MirrorLedger) -> Result<MirrorScan, String> {
    // A configured catalog that is missing gives only an untrusted directory listing (every
    // title would fall back); take no action rather than rename or remove from that view.
    if src.catalog.as_ref().is_some_and(|c| !c.is_file()) {
        return Ok(MirrorScan::default());
    }
    let sources = discover(src)?;
    let mut out = MirrorScan::default();
    let mut present = std::collections::BTreeSet::new();
    for source in sources {
        present.insert(source.origin_id.clone());
        let stamp = match try_stamp(&source.dir) {
            Ok(st) => st,
            // Deleted between discover and stamp: skip it this round (no action either way).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(e) => return Err(format!("stat {}: {e}", source.dir.display())),
        };
        let Some(entry) = ledger.data.entries.get(&source.origin_id) else {
            out.actions.push(MirrorAction::Create { source, stamp });
            continue;
        };
        match entry.state {
            MirrorState::Removed => out.actions.push(MirrorAction::Restore { source, stamp }),
            MirrorState::Rebuilding => out.actions.push(MirrorAction::Rebuild { source, stamp }),
            MirrorState::Unsupported if entry.stamp == Some(stamp) => out.unsupported.push((
                source.title.clone(),
                entry.error.clone().unwrap_or_default(),
            )),
            MirrorState::Unsupported => out.actions.push(MirrorAction::Rebuild { source, stamp }),
            MirrorState::Current if entry.stamp != Some(stamp) => {
                out.actions.push(MirrorAction::Rebuild { source, stamp })
            }
            MirrorState::Current if entry.title != source.title => {
                out.actions.push(MirrorAction::Rename { source })
            }
            MirrorState::Current => out.current += 1,
        }
    }
    for (id, entry) in &ledger.data.entries {
        if !present.contains(id) && entry.state != MirrorState::Removed {
            out.actions.push(MirrorAction::Remove {
                origin_id: id.clone(),
            });
        }
    }
    out.actions.sort_by(|a, b| a.origin_id().cmp(b.origin_id()));
    Ok(out)
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
