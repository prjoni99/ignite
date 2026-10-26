//! Kindle-authored typed text: stamps a page with a date or time as a real
//! [`sync_protocol::SyncText`] that reaches the server and the browser via `AddText`.
//!
//! This module only writes `texts.jsonl` and journals the op. It never touches the
//! framebuffer or the text overlay; the caller repaints. The page-open path already
//! rebuilds the overlay (`text_paint::load_texts_and_overlay`), so nothing here is on
//! the pen path.

use std::path::Path;

use notebook_core::CANONICAL_WIDTH;
use sync_protocol::{SyncText, TextFace, TextId};
use uuid::Uuid;

/// Left margin: 160 px in canonical units.
pub const STAMP_X: f32 = 1_344.0;
/// Below the 130 px chrome bar plus 50 px, in canonical units.
pub const STAMP_Y: f32 = 1_512.0;
/// Copied verbatim from the browser's `DEFAULT_TEXT_FONT_SIZE`
/// (`ignite/web/src/authoring/textDraft.ts:7`).
pub const STAMP_FONT_SIZE: f32 = 340.0;

/// Append `content` as a new text object on the page and journal an `AddText`.
pub fn stamp_page(
    nb_dir: &Path,
    notebook_local: &str,
    page_local: &str,
    content: &str,
) -> Result<SyncText, String> {
    if content.is_empty() {
        return Err("REFUSE TEXT: empty content".into());
    }
    let text = SyncText {
        text_id: TextId::from_uuid(Uuid::now_v7()).map_err(|e| e.to_string())?,
        x: STAMP_X,
        y: STAMP_Y,
        width: CANONICAL_WIDTH - 2.0 * STAMP_X,
        font_size: STAMP_FONT_SIZE,
        face: TextFace::DefaultSansV1,
        content: content.to_string(),
    };
    // Validate before preparing a ticket so a refused text never reaches the journal.
    text.validate().map_err(|e| format!("REFUSE TEXT: {e}"))?;
    let ticket = match crate::sync_outbound::with_global(|s| {
        s.prepare_add_text(notebook_local, page_local, &text)
    }) {
        None => None, // sync off: write locally, untracked
        Some(None) => return Err("local capture journal could not record this edit".into()),
        Some(Some(t)) => Some(t),
    };
    let dir = crate::store::page_dir(nb_dir, page_local);
    let written = (|| {
        let mut texts = crate::store::load_page_texts(&dir)?;
        texts.push(text.clone());
        crate::store::write_page_texts(&dir, &texts)
    })();
    // The append is a single atomic file write, so any error means nothing landed: abort.
    if let Some(t) = ticket {
        crate::sync_outbound::with_global(|s| {
            if written.is_ok() {
                s.commit(t)
            } else {
                s.abort(t)
            }
        });
    }
    written.map(|()| text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbk_test_kit::tmp;
    use crate::store::{load_page_texts, page_dir};

    fn page(label: &str) -> std::path::PathBuf {
        let nb = tmp(label);
        std::fs::create_dir_all(page_dir(&nb, "page-1")).unwrap();
        nb
    }

    #[test]
    fn stamp_page_appends_a_valid_text_and_round_trips() {
        let nb = page("stamp-rt");
        let t = stamp_page(&nb, "nb-1", "page-1", "2026-10-25").unwrap();
        t.validate().unwrap();
        let dir = page_dir(&nb, "page-1");
        let raw = std::fs::read_to_string(dir.join("texts.jsonl")).unwrap();
        assert_eq!(raw.lines().count(), 1);
        let loaded = load_page_texts(&dir).unwrap();
        assert_eq!(loaded, vec![t.clone()]);
        assert_eq!(t.content, "2026-10-25");
        assert_eq!(t.x, STAMP_X);
        assert_eq!(t.y, STAMP_Y);
        assert_eq!(t.font_size, STAMP_FONT_SIZE);
        assert_eq!(t.width, CANONICAL_WIDTH - 2.0 * STAMP_X);
        assert_eq!(t.face, TextFace::DefaultSansV1);
        let t2 = stamp_page(&nb, "nb-1", "page-1", "09:30").unwrap();
        let loaded = load_page_texts(&dir).unwrap();
        assert_eq!(loaded, vec![t, t2]);
        assert_eq!(
            std::fs::read_to_string(dir.join("texts.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn stamp_page_with_sync_off_writes_locally() {
        let nb = page("stamp-off");
        assert!(crate::sync_outbound::with_global(|_| ()).is_none());
        stamp_page(&nb, "nb-1", "page-1", "hello").unwrap();
        assert_eq!(load_page_texts(&page_dir(&nb, "page-1")).unwrap().len(), 1);
    }

    #[test]
    fn stamp_page_refuses_empty_content() {
        let nb = page("stamp-empty");
        let e = stamp_page(&nb, "nb-1", "page-1", "").unwrap_err();
        assert!(e.starts_with("REFUSE TEXT"), "{e}");
        assert!(load_page_texts(&page_dir(&nb, "page-1"))
            .unwrap()
            .is_empty());
    }
}
