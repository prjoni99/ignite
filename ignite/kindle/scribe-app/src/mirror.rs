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

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nbk_reader::{DecodedNotebook, NbkError};
use notebook_core::Stroke;
use serde::{Deserialize, Serialize};

use crate::folders::FolderStore;
use crate::home::AppView;
use crate::library::{
    allocate_fresh_notebook_id, read_notebook_manifest_read_only, rename_notebook_durable,
    LibraryRepository, RenameNotebookHooks,
};
use crate::nbk_import::{
    build, convert, discover_listed, halt, insert_pages_after, write_atomic, write_page_strokes,
    GlobalSync, ImportSources, ImportSync, ImportTarget, NoSync, SourceNotebook,
};
use crate::notebook::{delete_page_durable, DeletePageHooks, PersistedNotebookManifestV1};
use crate::runtime::{capture_folder_change, FolderCapture};
use crate::sync_outbound::DurableWaitError;

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
    /// Variant name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Rebuild { .. } => "rebuild",
            Self::Rename { .. } => "rename",
            Self::Restore { .. } => "restore",
            Self::Remove { .. } => "remove",
        }
    }

    pub fn origin_id(&self) -> &str {
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
    /// Listed sources whose directory or `nbk` could not be stat-ed this round (missing, or
    /// not a directory): neither acted on nor removed.
    pub skipped: usize,
}

/// A stat error on a listed source that skips the source instead of failing the scan:
/// its directory or `nbk` is missing, or the listed path is not a directory at all.
fn skip_source(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Classify every Amazon notebook against the ledger. Metadata only, never decodes.
/// `discover_listed`'s error is returned as-is. A missing catalog or notebooks root takes no
/// action, and a source the catalog lists but whose directory or `nbk` is missing is skipped:
/// only an origin the catalog no longer lists (or lists as deleted) is removed.
pub fn scan(src: &ImportSources, ledger: &MirrorLedger) -> Result<MirrorScan, String> {
    // A configured catalog that is missing gives only an untrusted directory listing (every
    // title would fall back); take no action rather than rename or remove from that view.
    // A missing notebooks root (storage not mounted, mid-sync) is the same: pause, never remove.
    if src.catalog.as_ref().is_some_and(|c| !c.is_file()) || !src.notebooks_root.is_dir() {
        return Ok(MirrorScan::default());
    }
    let sources = discover_listed(src)?;
    let mut out = MirrorScan::default();
    let mut present = std::collections::BTreeSet::new();
    for source in sources {
        present.insert(source.origin_id.clone());
        let stamp = match try_stamp(&source.dir) {
            Ok(st) => st,
            // Listed but its directory or `nbk` is missing (or deleted between discover and
            // stamp), or the listed path is a file rather than a directory: present but
            // skipped this round, no action either way. One bad catalog row never pauses
            // every other mirror.
            Err(e) if skip_source(&e) => {
                out.skipped += 1;
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
        // Unlike rename: only a refusal (nothing durable happened) aborts. Any other failure
        // leaves the ticket unresolved for startup recovery, exactly as the runtime does.
        if let Some(t) = ticket {
            match &r {
                Ok(_) => {
                    crate::sync_outbound::with_global(|s| s.commit(t));
                }
                Err(e) if e.starts_with("REFUSE PAGE DELETE") => {
                    crate::sync_outbound::with_global(|s| s.abort(t));
                }
                Err(_) => {}
            }
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
        // Like the runtime: abort on any error or an unchanged title (no delete-style recovery).
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
    // Accepted edge: if this write fails, the folder was already created and captured, so a
    // synced (harmless) folder is left that the next call will not reuse.
    ledger.set_folders(amazon, removed)?;
    Ok(id)
}

// ------------------------------------------------------------------------------ engine

/// Immutable provenance written into each mirror notebook; its presence makes the notebook
/// read-only by convention.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorOriginV1 {
    pub v: u32,
    pub origin_kind: String,
    pub origin_id: String,
    pub mirrored_unix_ms: u64,
}

/// Where mirror notebooks live: the import target plus the library's folders.
pub struct MirrorTarget<'a> {
    pub import: ImportTarget<'a>,
    pub folders: &'a mut FolderStore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorOutcome {
    Done,
    /// Transient: retried on a later scan; the ledger is untouched.
    Skipped(String),
    Failed(String),
    /// Sync faulted/stopped or the wait was cancelled: the caller stops the run.
    Halted(String),
}

/// Forwards to the caller's sync and remembers whether a flush failed, so any error out of
/// the engine (including `nbk_import::build`'s) is classified as a halt exactly when sync
/// stopped.
struct Watch<'a> {
    inner: &'a dyn MirrorSync,
    halted: Cell<bool>,
}

impl ImportSync for Watch<'_> {
    fn notebook_created(&self, library: &LibraryRepository, id: &str, title: &str) {
        self.inner.notebook_created(library, id, title);
    }
    fn page_created(&self, dir: &Path, nb: &str, page: &str, after: &str) {
        self.inner.page_created(dir, nb, page, after);
    }
    fn strokes_committed(&self, nb: &str, page: &str, strokes: &[Stroke]) {
        self.inner.strokes_committed(nb, page, strokes);
    }
    fn flush(&self, cancel: &AtomicBool) -> Result<(), DurableWaitError> {
        let r = self.inner.flush(cancel);
        if r.is_err() {
            self.halted.set(true);
        }
        r
    }
    fn delete_notebook(&self, library: &mut LibraryRepository, id: &str) -> Result<(), String> {
        self.inner.delete_notebook(library, id)
    }
}

impl MirrorSync for Watch<'_> {
    fn delete_page(
        &self,
        nb_dir: &Path,
        manifest: &mut PersistedNotebookManifestV1,
        notebook_id: &str,
        page_id: &str,
    ) -> Result<(), String> {
        self.inner
            .delete_page(nb_dir, manifest, notebook_id, page_id)
    }
    fn rename_notebook(
        &self,
        library: &mut LibraryRepository,
        notebook_id: &str,
        title: &str,
    ) -> Result<(), String> {
        self.inner.rename_notebook(library, notebook_id, title)
    }
    fn folder_changed(&self, folders: &FolderStore, change: FolderCapture) {
        self.inner.folder_changed(folders, change);
    }
}

/// Decode an Amazon notebook directory (in-memory copies; the source is never written).
pub fn kindle_decode(dir: &Path) -> Result<DecodedNotebook, NbkError> {
    nbk_reader::read_source(dir).and_then(|s| nbk_reader::decode_snapshot(&s))
}

/// The entry's notebook, when it is in the library and its marker names this origin. An id
/// reserved by a create that never happened may since belong to a user notebook (no marker);
/// that one is never touched. A marker that is present but unreadable is an error, never
/// "absent": treating it as absent would build a second mirror of the same origin.
fn mirror_notebook(
    library: &LibraryRepository,
    e: &MirrorEntryV1,
) -> Result<Option<String>, String> {
    let Some(dest) = e.dest_notebook_id.as_ref() else {
        return Ok(None);
    };
    if !library.manifest().notebook_ids.contains(dest) {
        return Ok(None);
    }
    let path = library.notebook_dir(dest).join(MIRROR_MARKER);
    let b = match fs::read(&path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("read {}: {err}", path.display())),
    };
    let m: MirrorOriginV1 = serde_json::from_slice(&b)
        .map_err(|err| format!("unreadable mirror marker {}: {err}", path.display()))?;
    Ok((m.origin_id == e.origin_id).then(|| dest.clone()))
}

/// A `Rebuilding` entry's half-replaced notebook goes (synced delete) before the entry is
/// rewritten as anything other than `Rebuilding`, so recovery never loses sight of it.
fn drop_partial(
    library: &mut LibraryRepository,
    e: &MirrorEntryV1,
    sync: &dyn MirrorSync,
) -> Result<(), String> {
    if e.state != MirrorState::Rebuilding {
        return Ok(());
    }
    if let Some(dest) = mirror_notebook(library, e)? {
        sync.delete_notebook(library, &dest)
            .map_err(|why| format!("remove partial mirror {dest}: {why}"))?;
    }
    Ok(())
}

/// Perform one action. `decode` is injected so tests can hand in a [`DecodedNotebook`]
/// without private samples. Amazon state is only ever read (through `decode`).
pub(crate) fn apply_one(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    action: &MirrorAction,
    decode: &dyn Fn(&Path) -> Result<DecodedNotebook, NbkError>,
    sync: &dyn MirrorSync,
) -> MirrorOutcome {
    let _admit = match crate::sync_verified::note_library_mutation(t.import.library.root()) {
        Ok(a) => a,
        Err(e) => return MirrorOutcome::Failed(e),
    };
    let w = Watch {
        inner: sync,
        halted: Cell::new(false),
    };
    let r = (|| {
        // Sync must be healthy and caught up before anything changes.
        w.flush(t.import.cancel)
            .map_err(|e| halt(e, "mirror start"))?;
        match action {
            MirrorAction::Create { source, stamp }
            | MirrorAction::Rebuild { source, stamp }
            | MirrorAction::Restore { source, stamp } => {
                mirror_content(t, ledger, source, *stamp, decode, &w)
            }
            MirrorAction::Rename { source } => rename(t, ledger, source, &w),
            MirrorAction::Remove { origin_id } => remove(t, ledger, origin_id, &w),
        }
    })();
    match r {
        Ok(o) => o,
        // A Rebuilding entry stays Rebuilding: startup recovery removes the partial notebook.
        Err(e) if w.halted.get() => MirrorOutcome::Halted(e),
        Err(e) => MirrorOutcome::Failed(e),
    }
}

/// Create, rebuild or restore: decode, then (unless the content is unchanged) rebuild the
/// mirror notebook in place, or build a fresh one when none is left.
fn mirror_content(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    source: &SourceNotebook,
    stamp: SourceStamp,
    decode: &dyn Fn(&Path) -> Result<DecodedNotebook, NbkError>,
    sync: &Watch<'_>,
) -> Result<MirrorOutcome, String> {
    let prev = ledger.data.entries.get(&source.origin_id).cloned();
    // Decode and convert fully in memory; nothing is written before this succeeds.
    let decoded = match decode(&source.dir) {
        Ok(d) => d,
        Err(e) => {
            return match classify_decode_error(&e) {
                DecodeFailure::Transient(why) => Ok(MirrorOutcome::Skipped(why)),
                DecodeFailure::Permanent(why) => {
                    unsupported(t, ledger, prev, source, stamp, why, sync)
                }
            }
        }
    };
    let pages = match convert(&decoded) {
        Ok((p, _)) if p.is_empty() => {
            let why = "NBK_UNSUPPORTED: notebook has no pages".to_string();
            return unsupported(t, ledger, prev, source, stamp, why, sync);
        }
        Ok((p, _)) => p,
        Err(why) => return unsupported(t, ledger, prev, source, stamp, why, sync),
    };
    let hash = content_hash(&decoded);
    let live = match &prev {
        Some(e) => mirror_notebook(t.import.library, e)?,
        None => None,
    };

    if let (Some(dest), Some(p)) = (&live, &prev) {
        if p.content_sha256.as_deref() == Some(hash.as_str()) {
            // Same drawn content: no page is touched.
            let entry = MirrorEntryV1 {
                title: source.title.clone(),
                dest_notebook_id: Some(dest.clone()),
                stamp: Some(stamp),
                ..p.clone()
            };
            return finish(t, ledger, entry, sync);
        }
    }

    // Durable intent before the first native mutation.
    let in_place = live.is_some();
    let dest = live.unwrap_or_else(|| allocate_fresh_notebook_id(t.import.library));
    let mut entry = MirrorEntryV1 {
        origin_id: source.origin_id.clone(),
        title: source.title.clone(),
        state: MirrorState::Rebuilding,
        dest_notebook_id: Some(dest.clone()),
        stamp: prev.as_ref().and_then(|p| p.stamp),
        content_sha256: None,
        pages: prev.as_ref().map_or(0, |p| p.pages),
        error: None,
        updated_unix_ms: now_ms(),
    };
    ledger.put(entry.clone())?;

    entry.pages = if in_place {
        replace_pages(t, &dest, &pages, sync)?
    } else {
        let origin = MirrorOriginV1 {
            v: 1,
            origin_kind: ORIGIN_KIND.into(),
            origin_id: source.origin_id.clone(),
            mirrored_unix_ms: now_ms(),
        };
        let bytes = serde_json::to_vec_pretty(&origin).map_err(|e| e.to_string())?;
        build(
            &mut t.import,
            &dest,
            &source.title,
            &pages,
            (MIRROR_MARKER, &bytes),
            sync,
        )?;
        pages.len()
    };
    entry.stamp = Some(stamp);
    entry.content_sha256 = Some(hash);
    finish(t, ledger, entry, sync)
}

/// New pages after the current last page, their strokes, then the old pages in order: the
/// notebook never has fewer than one page. Returns the final page count.
fn replace_pages(
    t: &mut MirrorTarget<'_>,
    dest: &str,
    pages: &[Vec<Stroke>],
    sync: &dyn MirrorSync,
) -> Result<usize, String> {
    let lib = &mut *t.import.library;
    let cancel = t.import.cancel;
    let nb_dir = lib.notebook_dir(dest);
    let mut manifest = read_notebook_manifest_read_only(&nb_dir)?;
    let old = manifest.page_ids.clone();
    let fresh = insert_pages_after(lib, dest, &mut manifest, pages.len(), cancel, sync)?;
    for (page_id, strokes) in fresh.iter().zip(pages) {
        write_page_strokes(&nb_dir, dest, page_id, strokes, cancel, sync)?;
    }
    for page_id in &old {
        let r = sync.delete_page(&nb_dir, &mut manifest, dest, page_id);
        lib.set_cached_page_count(dest, manifest.page_ids.len());
        r?;
        sync.flush(cancel).map_err(|e| halt(e, "page delete"))?;
    }
    Ok(manifest.page_ids.len())
}

/// File the notebook into `which` (unless it is already there).
fn file_into(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    notebook_id: &str,
    which: Which,
    sync: &dyn MirrorSync,
) -> Result<(), String> {
    let live = t.import.library.manifest().notebook_ids.clone();
    let folder = ensure_folder(ledger, t.folders, &live, which, sync)?;
    if t.folders.folder_of(notebook_id) == Some(folder.as_str()) {
        return Ok(());
    }
    t.folders.set_folder(notebook_id, Some(&folder), &live)?;
    sync.folder_changed(
        t.folders,
        FolderCapture::Filed {
            notebook: notebook_id.into(),
            folder: Some(folder),
        },
    );
    Ok(())
}

/// File the notebook under Amazon with the source title, then mark the entry `Current` once
/// every capture is durable.
fn finish(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    mut entry: MirrorEntryV1,
    sync: &dyn MirrorSync,
) -> Result<MirrorOutcome, String> {
    let dest = entry
        .dest_notebook_id
        .clone()
        .ok_or("mirror entry has no notebook")?;
    file_into(t, ledger, &dest, Which::Amazon, sync)?;
    if t.import.library.title(&dest).ok() != Some(entry.title.as_str()) {
        sync.rename_notebook(t.import.library, &dest, &entry.title)?;
    }
    sync.flush(t.import.cancel)
        .map_err(|e| halt(e, "mirror finish"))?;
    entry.state = MirrorState::Current;
    entry.error = None;
    entry.updated_unix_ms = now_ms();
    ledger.put(entry)?;
    Ok(MirrorOutcome::Done)
}

/// A source that will not decode as-is: remember its stamp and why (so it is not retried
/// until it changes); a complete mirror notebook is kept as it is, a half-rebuilt one removed.
fn unsupported(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    prev: Option<MirrorEntryV1>,
    source: &SourceNotebook,
    stamp: SourceStamp,
    why: String,
    sync: &dyn MirrorSync,
) -> Result<MirrorOutcome, String> {
    let prev = match prev {
        Some(p) if p.state == MirrorState::Rebuilding => {
            drop_partial(t.import.library, &p, sync)?;
            Some(MirrorEntryV1 {
                dest_notebook_id: None,
                content_sha256: None,
                pages: 0,
                ..p
            })
        }
        p => p,
    };
    let base = prev.unwrap_or_else(|| MirrorEntryV1 {
        origin_id: source.origin_id.clone(),
        title: source.title.clone(),
        state: MirrorState::Unsupported,
        dest_notebook_id: None,
        stamp: None,
        content_sha256: None,
        pages: 0,
        error: None,
        updated_unix_ms: 0,
    });
    ledger.put(MirrorEntryV1 {
        state: MirrorState::Unsupported,
        stamp: Some(stamp),
        error: Some(why.clone()),
        updated_unix_ms: now_ms(),
        ..base
    })?;
    Ok(MirrorOutcome::Failed(why))
}

fn rename(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    source: &SourceNotebook,
    sync: &dyn MirrorSync,
) -> Result<MirrorOutcome, String> {
    let Some(mut e) = ledger.data.entries.get(&source.origin_id).cloned() else {
        return Err(format!("no mirror entry for {}", source.origin_id));
    };
    if let Some(dest) = mirror_notebook(t.import.library, &e)? {
        if t.import.library.title(&dest).ok() != Some(source.title.as_str()) {
            sync.rename_notebook(t.import.library, &dest, &source.title)?;
            sync.flush(t.import.cancel).map_err(|e| halt(e, "rename"))?;
        }
    }
    e.title = source.title.clone();
    e.updated_unix_ms = now_ms();
    ledger.put(e)?;
    Ok(MirrorOutcome::Done)
}

/// The source is gone: its mirror moves to the Removed folder (never deleted); an entry that
/// never built anything, or whose rebuild was left half-done (that partial is removed), is
/// forgotten.
fn remove(
    t: &mut MirrorTarget<'_>,
    ledger: &mut MirrorLedger,
    origin_id: &str,
    sync: &dyn MirrorSync,
) -> Result<MirrorOutcome, String> {
    let Some(mut e) = ledger.data.entries.get(origin_id).cloned() else {
        return Ok(MirrorOutcome::Done);
    };
    if e.state == MirrorState::Rebuilding {
        drop_partial(t.import.library, &e, sync)?;
        ledger.remove(origin_id)?;
        return Ok(MirrorOutcome::Done);
    }
    if e.dest_notebook_id.is_none() {
        ledger.remove(origin_id)?;
        return Ok(MirrorOutcome::Done);
    }
    if let Some(dest) = mirror_notebook(t.import.library, &e)? {
        file_into(t, ledger, &dest, Which::Removed, sync)?;
        sync.flush(t.import.cancel).map_err(|e| halt(e, "remove"))?;
    }
    e.state = MirrorState::Removed;
    e.updated_unix_ms = now_ms();
    ledger.put(e)?;
    Ok(MirrorOutcome::Done)
}

/// Startup: every `Rebuilding` entry's notebook is removed through the synced delete path and
/// the entry dropped (so the next scan recreates it). Runs before any UI, next to the
/// `nbk_import` recovery. Returns how many entries were recovered.
pub(crate) fn recover_interrupted(
    library: &mut LibraryRepository,
    sync: &dyn MirrorSync,
) -> Result<usize, String> {
    let mut ledger = MirrorLedger::open(library.root())?;
    let stuck: Vec<MirrorEntryV1> = ledger
        .data
        .entries
        .values()
        .filter(|e| e.state == MirrorState::Rebuilding)
        .cloned()
        .collect();
    let mut recovered = 0;
    let mut errors = Vec::new();
    for e in stuck {
        // An unreadable marker is reported and the entry kept (retried next start).
        if let Err(why) = drop_partial(library, &e, sync) {
            errors.push(format!("{}: {why}", e.origin_id));
            continue;
        }
        match ledger.remove(&e.origin_id) {
            Ok(()) => recovered += 1,
            Err(why) => errors.push(format!("{}: {why}", e.origin_id)),
        }
    }
    if errors.is_empty() {
        Ok(recovered)
    } else {
        Err(format!("mirror recovery: {}", errors.join("; ")))
    }
}

// ------------------------------------------------------------------------- scheduling

/// A queued rebuild pass: one action is applied per structural safe point.
#[derive(Debug, Default)]
pub struct MirrorRun {
    pub queue: VecDeque<MirrorAction>,
    pub done: usize,
    /// `(origin_id, detail)` of actions that failed this run.
    pub failed: Vec<(String, String)>,
}

/// When the last scan ran and the upload batch counter it saw.
#[derive(Debug, Default)]
pub struct MirrorClock {
    pub last_scan: Option<Instant>,
    pub last_batches: u64,
}

/// Apply the Preferences toggle to the scheduler. Off drops any in-progress run (each step is
/// durable in the ledger, so a later run resumes); on clears the clock so a scan runs at the
/// next structural safe point. Returns true when a run was cancelled.
pub fn apply_mirror_toggle(
    enabled: bool,
    run: &mut Option<MirrorRun>,
    clock: &mut MirrorClock,
) -> bool {
    if enabled {
        clock.last_scan = None;
        false
    } else {
        run.take().is_some()
    }
}

/// Rescan this often even when no upload batch completed.
pub const IDLE_RESCAN: Duration = Duration::from_secs(15 * 60);

/// Should a scan start now? launch (last_scan None), upload batch count advanced, or idle timer elapsed.
pub fn scan_due(clock: &MirrorClock, now: Instant, batches: u64) -> bool {
    match clock.last_scan {
        None => true,
        Some(last) => {
            batches > clock.last_batches || now.saturating_duration_since(last) >= IDLE_RESCAN
        }
    }
}

/// May a rebuild step run now? Home or Tools only, no import run, sync linked and capture-ready.
pub fn step_allowed(view: AppView, import_running: bool, sync_linked: bool) -> bool {
    matches!(view, AppView::Home | AppView::Tools) && !import_running && sync_linked
}

/// May a scan start now? Same gate as a step: the scan and its ledger fsync never run on the
/// main thread between strokes with a page open.
pub fn scan_allowed(view: AppView, import_running: bool, sync_linked: bool) -> bool {
    step_allowed(view, import_running, sync_linked)
}

// ---------------------------------------------------------------------------- summary

/// What the Tools screen shows about the mirror. Built from the ledger without touching the
/// Kindle's files (no stat, no decode).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorSummary {
    pub enabled: bool,
    /// Ledger entries in step with Amazon.
    pub current: usize,
    /// `Rebuilding` entries plus actions still queued in the running pass.
    pub pending: usize,
    pub last_scan_unix_ms: Option<u64>,
    pub last_scan_error: Option<String>,
    /// `(title, error)` of `Unsupported` entries, in origin-id order.
    pub unsupported: Vec<(String, String)>,
    /// When the summary was taken (the "last scan N ago" reference point).
    pub now_unix_ms: u64,
}

/// Summarize the ledger for the Tools screen. `queued` is the running pass's queue length.
pub fn summary(
    ledger: &MirrorLedger,
    enabled: bool,
    queued: usize,
    now_unix_ms: u64,
) -> MirrorSummary {
    let d = &ledger.data;
    let count = |s: MirrorState| d.entries.values().filter(|e| e.state == s).count();
    MirrorSummary {
        enabled,
        current: count(MirrorState::Current),
        pending: count(MirrorState::Rebuilding) + queued,
        last_scan_unix_ms: d.last_scan_unix_ms,
        last_scan_error: d.last_scan_error.clone(),
        unsupported: d
            .entries
            .values()
            .filter(|e| e.state == MirrorState::Unsupported)
            .map(|e| (e.title.clone(), e.error.clone().unwrap_or_default()))
            .collect(),
        now_unix_ms,
    }
}

/// Wall-clock now in Unix milliseconds (0 if the clock is before the epoch).
pub fn unix_now_ms() -> u64 {
    now_ms()
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
