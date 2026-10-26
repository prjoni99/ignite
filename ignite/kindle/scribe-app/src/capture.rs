//! Quick capture: a top-level `Inbox` notebook (blank template) that the Home `CAPTURE` tile or
//! the side button opens on a fresh, time-stamped page. The ledger
//! (`<data-dir>/.scribe-capture/capture.json`) remembers the Inbox by local id; a renamed Inbox
//! is still used, a deleted one is created again. Creation is injected so the test kit can
//! drive it; the runtime's closure adds the sync capture.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app_ui::UiCommand;
use crate::library::LibraryRepository;
use crate::nbk_import::write_atomic;
use crate::notebook::PersistedNotebookManifestV1;

pub const CAPTURE_DIR: &str = ".scribe-capture";
pub const CAPTURE_FILE: &str = "capture.json";
pub const INBOX_TITLE: &str = "Inbox";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLedgerV1 {
    pub v: u32,
    /// Local notebook id of the Inbox.
    pub inbox: Option<String>,
}

/// The capture ledger. A corrupt one fails closed: which notebook is the Inbox cannot be
/// proven, and guessing would create duplicates.
#[derive(Debug)]
pub struct CaptureLedger {
    path: PathBuf,
    pub data: CaptureLedgerV1,
}

impl CaptureLedger {
    pub fn path_for(data_dir: &Path) -> PathBuf {
        data_dir.join(CAPTURE_DIR).join(CAPTURE_FILE)
    }

    /// A missing ledger is empty; an unreadable one or any version other than 1 is refused.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let path = Self::path_for(data_dir);
        let data = match fs::read(&path) {
            Ok(b) => {
                let d: CaptureLedgerV1 = serde_json::from_slice(&b)
                    .map_err(|e| format!("REFUSE CAPTURE: unreadable capture ledger: {e}"))?;
                if d.v != 1 {
                    return Err(format!("REFUSE CAPTURE: capture ledger version {}", d.v));
                }
                d
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                CaptureLedgerV1 { v: 1, inbox: None }
            }
            Err(e) => return Err(format!("REFUSE CAPTURE: read capture ledger: {e}")),
        };
        Ok(Self { path, data })
    }

    /// Record `inbox` atomically; memory changes only once the write landed.
    pub fn put(&mut self, inbox: &str) -> Result<(), String> {
        let next = CaptureLedgerV1 {
            v: 1,
            inbox: Some(inbox.to_string()),
        };
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
        write_atomic(&self.path, &bytes).map_err(|e| format!("capture ledger write: {e}"))?;
        self.data = next;
        Ok(())
    }
}

/// Where the runtime returns after a capture: the page it left (main-loop local, one deep).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnSlot {
    pub notebook_id: String,
    pub page_id: String,
}

/// The Inbox's (id, created). The recorded id is used while the library still has it (a
/// rename keeps it); otherwise `create` makes a new one, which is recorded only after that
/// durable create returned.
pub fn resolve_inbox(
    ledger: &mut CaptureLedger,
    library: &mut LibraryRepository,
    create: &mut dyn FnMut(&mut LibraryRepository) -> Result<String, String>,
) -> Result<(String, bool), String> {
    if let Some(id) = &ledger.data.inbox {
        if library.manifest().notebook_ids.contains(id) {
            return Ok((id.clone(), false));
        }
    }
    let id = create(library)?;
    // Accepted edge (as the daily ledger's): if this write fails the Inbox exists unrecorded
    // and the next capture creates another.
    ledger.put(&id)?;
    Ok((id, true))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapturePage {
    /// A just-created Inbox: its first page is the capture.
    ReuseFirst,
    /// The Inbox's only page, still untouched (see [`reuse_only_page`]): reused as it is.
    /// Its stamp is kept and no second one is written.
    ReuseUntouched(String),
    /// Append after this (the last) page.
    InsertAfter(String),
}

/// `only_page_untouched` is [`reuse_only_page`] for the Inbox's page (false when unknown).
pub fn next_capture_page(
    manifest: &PersistedNotebookManifestV1,
    created: bool,
    only_page_untouched: bool,
) -> CapturePage {
    match manifest.page_ids.as_slice() {
        _ if created => CapturePage::ReuseFirst,
        [only] if only_page_untouched => CapturePage::ReuseUntouched(only.clone()),
        [.., last] => CapturePage::InsertAfter(last.clone()),
        [] => CapturePage::ReuseFirst,
    }
}

/// The Inbox has exactly one page and it passes the untouched test (no stroke ever written,
/// no link, at most the one stamp): the next capture reuses it instead of appending.
pub fn reuse_only_page(
    manifest: &PersistedNotebookManifestV1,
    ever_strokes: usize,
    link_count: usize,
    text_count: usize,
) -> bool {
    manifest.page_ids.len() == 1 && ever_strokes == 0 && link_count == 0 && text_count <= 1
}

/// The page a capture opened goes away when it is left untouched: no stroke was ever written
/// on it (`ever_strokes`, the store's stroke ids, so a drawn-then-undone page is kept), it has
/// no link, and no text beyond the one stamp the capture wrote. The notebook's only page
/// always stays.
pub fn should_discard_on_leave(
    ever_strokes: usize,
    page_count: usize,
    link_count: usize,
    text_count: usize,
) -> bool {
    ever_strokes == 0 && page_count > 1 && link_count == 0 && text_count <= 1
}

/// Leaving the view `current` (notebook id, page id): the capture is over either way (the
/// slot is cleared); true only when `current` is the very page this capture opened, the
/// only page the discard rule may touch.
pub fn leaving(capture_page: &mut Option<(String, String)>, current: Option<(&str, &str)>) -> bool {
    match (capture_page.take(), current) {
        (Some((nb, page)), Some((cur_nb, cur_page))) => nb == cur_nb && page == cur_page,
        _ => false,
    }
}

/// The side button's capture gesture: return from an Inbox page, capture from anywhere else.
pub fn capture_command_for(on_inbox: bool) -> UiCommand {
    if on_inbox {
        UiCommand::CaptureReturn
    } else {
        UiCommand::Capture
    }
}

/// The ledger names `notebook_id` as the Inbox (an unreadable ledger names nothing).
pub fn is_inbox(data_dir: &Path, notebook_id: &str) -> bool {
    CaptureLedger::open(data_dir)
        .map(|l| l.data.inbox.as_deref() == Some(notebook_id))
        .unwrap_or(false)
}

#[cfg(all(test, unix))]
#[path = "capture_tests.rs"]
mod tests;
