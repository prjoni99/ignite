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

use crate::folders::FolderStore;
use crate::library::{rename_notebook_durable, LibraryRepository, RenameNotebookHooks};
use crate::nbk_import::{
    discover, write_atomic, GlobalSync, ImportSources, ImportSync, NoSync, SourceNotebook,
};
use crate::notebook::{delete_page_durable, DeletePageHooks, PersistedNotebookManifestV1};
use crate::runtime::{capture_folder_change, FolderCapture};

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

/// How a failed decode should be treated by the rebuild engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeFailure {
    /// Retry on a later scan; the source may become readable.
    Transient(String),
    /// The source will not decode as-is; do not retry until it changes.
    Permanent(String),
}

pub fn classify_decode_error(e: &nbk_reader::NbkError) -> DecodeFailure {
    use nbk_reader::NbkError::*;
    match e {
        Io(_) | SourceBusy(_) | SourceChanged(_) | PendingActions { .. } => {
            DecodeFailure::Transient(e.to_string())
        }
        Container(_) | Unsupported(_) => DecodeFailure::Permanent(e.to_string()),
    }
}

fn put_len(h: &mut sha2::Sha256, n: usize) {
    use sha2::Digest;
    h.update((n as u64).to_le_bytes());
}

fn put_i64s(h: &mut sha2::Sha256, v: &[i64]) {
    use sha2::Digest;
    put_len(h, v.len());
    for x in v {
        h.update(x.to_le_bytes());
    }
}

/// Deterministic lowercase-hex SHA-256 over the drawn content of a decoded notebook.
/// Diagnostics, fallbacks, template ids and `book_id` do not affect it.
pub fn content_hash(nb: &nbk_reader::DecodedNotebook) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    put_len(&mut h, nb.pages.len());
    for p in &nb.pages {
        put_len(&mut h, p.page_id.len());
        h.update(p.page_id.as_bytes());
        h.update(p.width.to_le_bytes());
        h.update(p.height.to_le_bytes());
        put_len(&mut h, p.strokes.len());
        for s in &p.strokes {
            put_i64s(&mut h, &s.xs);
            put_i64s(&mut h, &s.ys);
            put_i64s(&mut h, s.pressure.as_deref().unwrap_or(&[]));
            for opt in [s.brush_type, s.color] {
                match opt {
                    Some(v) => {
                        h.update([1u8]);
                        h.update(v.to_le_bytes());
                    }
                    None => h.update([0u8]),
                }
            }
            match s.thickness {
                Some(t) => {
                    h.update([1u8]);
                    h.update(t.to_bits().to_le_bytes());
                }
                None => h.update([0u8]),
            }
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------------------ sync hooks

/// Mirror mutations beyond what [`ImportSync`] covers. `GlobalSync` follows the runtime's
/// prepare/commit discipline; `NoSync` is the plain durable write.
pub(crate) trait MirrorSync: ImportSync {
    fn delete_page(
        &self,
        nb_dir: &Path,
        manifest: &mut PersistedNotebookManifestV1,
        notebook_id: &str,
        page_id: &str,
    ) -> Result<(), String>;
    fn rename_notebook(
        &self,
        library: &mut LibraryRepository,
        notebook_id: &str,
        title: &str,
    ) -> Result<(), String>;
    fn folder_changed(&self, folders: &FolderStore, change: FolderCapture);
}

impl MirrorSync for NoSync {
    fn delete_page(
        &self,
        nb_dir: &Path,
        manifest: &mut PersistedNotebookManifestV1,
        _: &str,
        page_id: &str,
    ) -> Result<(), String> {
        delete_page_durable(nb_dir, manifest, page_id, &DeletePageHooks::default()).map(|_| ())
    }
    fn rename_notebook(
        &self,
        library: &mut LibraryRepository,
        notebook_id: &str,
        title: &str,
    ) -> Result<(), String> {
        rename_notebook_durable(library, notebook_id, title, &RenameNotebookHooks::default())
            .map(|_| ())
    }
    fn folder_changed(&self, _: &FolderStore, _: FolderCapture) {}
}

impl MirrorSync for GlobalSync {
    fn delete_page(
        &self,
        nb_dir: &Path,
        manifest: &mut PersistedNotebookManifestV1,
        notebook_id: &str,
        page_id: &str,
    ) -> Result<(), String> {
        // None = sync off (no ticket); Some(None) = the journal could not record the edit.
        let ticket = match crate::sync_outbound::with_global(|s| {
            s.prepare_delete_page(notebook_id, page_id)
        }) {
            None => None,
            Some(None) => return Err("local capture journal could not record this edit".into()),
            Some(Some(t)) => Some(t),
        };
        let r = delete_page_durable(nb_dir, manifest, page_id, &DeletePageHooks::default());
        if let Some(t) = ticket {
            let ok = r.is_ok();
            crate::sync_outbound::with_global(|s| if ok { s.commit(t) } else { s.abort(t) });
        }
        r.map(|_| ())
    }
    fn rename_notebook(
        &self,
        library: &mut LibraryRepository,
        notebook_id: &str,
        title: &str,
    ) -> Result<(), String> {
        let ticket = if library.title(notebook_id).ok() != Some(title) {
            match crate::sync_outbound::with_global(|s| {
                s.prepare_rename_notebook(notebook_id, title)
            }) {
                None => None,
                Some(None) => return Err("local capture journal could not record this edit".into()),
                Some(Some(t)) => Some(t),
            }
        } else {
            None
        };
        let r =
            rename_notebook_durable(library, notebook_id, title, &RenameNotebookHooks::default());
        if let Some(t) = ticket {
            let changed = r.as_ref().is_ok_and(|c| c.changed);
            crate::sync_outbound::with_global(|s| if changed { s.commit(t) } else { s.abort(t) });
        }
        r.map(|_| ())
    }
    fn folder_changed(&self, folders: &FolderStore, change: FolderCapture) {
        capture_folder_change(folders, change);
    }
}

// ------------------------------------------------------------------------------ folders

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Amazon,
    Removed,
}

/// The local folder for `which`: the ledger-recorded id while the folder still exists (a user
/// rename keeps it), otherwise a freshly created one (the user deleted it), captured for sync
/// and recorded in the ledger.
pub(crate) fn ensure_folder(
    ledger: &mut MirrorLedger,
    folders: &mut FolderStore,
    live: &[String],
    which: Which,
    sync: &dyn MirrorSync,
) -> Result<String, String> {
    let (recorded, title) = match which {
        Which::Amazon => (&ledger.data.amazon_folder, AMAZON_FOLDER_TITLE),
        Which::Removed => (&ledger.data.removed_folder, REMOVED_FOLDER_TITLE),
    };
    if let Some(id) = recorded {
        if folders.title(id).is_some() {
            return Ok(id.clone());
        }
    }
    let id = folders.create(title, live)?;
    sync.folder_changed(folders, FolderCapture::Created(id.clone()));
    let (amazon, removed) = match which {
        Which::Amazon => (Some(id.clone()), ledger.data.removed_folder.clone()),
        Which::Removed => (ledger.data.amazon_folder.clone(), Some(id.clone())),
    };
    ledger.set_folders(amazon, removed)?;
    Ok(id)
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
