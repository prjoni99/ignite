use super::*;
use crate::nbk_test_kit::*;
use std::fs;

const ID_A: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const ID_B: &str = "bbbbbbbb-0000-0000-0000-000000000002";
const ID_C: &str = "cccccccc-0000-0000-0000-000000000003";
const ID_D: &str = "dddddddd-0000-0000-0000-000000000004";
const ID_E: &str = "eeeeeeee-0000-0000-0000-000000000005";

fn entry(id: &str, state: MirrorState) -> MirrorEntryV1 {
    MirrorEntryV1 {
        origin_id: id.into(),
        title: "One".into(),
        state,
        dest_notebook_id: Some(format!("dest-{id}")),
        stamp: None,
        content_sha256: None,
        pages: 0,
        error: None,
        updated_unix_ms: 1,
    }
}

fn entry_at(k: &Kindle, id: &str, title: &str, state: MirrorState) -> MirrorEntryV1 {
    MirrorEntryV1 {
        title: title.into(),
        stamp: Some(stamp_source(&k.root.join(id)).unwrap()),
        ..entry(id, state)
    }
}

#[test]
fn ledger_round_trips_and_a_corrupt_ledger_fails_closed() {
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry("a", MirrorState::Current)).unwrap();
    l.set_folders(Some("f1".into()), Some("f2".into())).unwrap();
    l.note_scan(Some("boom".into())).unwrap();
    assert_eq!(MirrorLedger::open(s.lib.root()).unwrap().data, l.data);
    assert_eq!(l.data.v, 1);
    assert!(l.data.last_scan_unix_ms.is_some());
    l.remove("a").unwrap();
    assert!(MirrorLedger::open(s.lib.root())
        .unwrap()
        .data
        .entries
        .is_empty());
    fs::write(MirrorLedger::path_for(s.lib.root()), b"{").unwrap();
    assert!(MirrorLedger::open(s.lib.root())
        .unwrap_err()
        .contains("REFUSE MIRROR"));
    fs::write(
        MirrorLedger::path_for(s.lib.root()),
        br#"{"v":2,"entries":{},"amazon_folder":null,"removed_folder":null,"last_scan_unix_ms":null,"last_scan_error":null}"#,
    )
    .unwrap();
    assert!(MirrorLedger::open(s.lib.root())
        .unwrap_err()
        .contains("REFUSE MIRROR"));
}

#[test]
fn a_failed_ledger_write_keeps_memory_equal_to_disk() {
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry("a", MirrorState::Current)).unwrap();
    let before = l.data.clone();
    // A directory where the temp file goes makes every write fail.
    let tmp = MirrorLedger::path_for(s.lib.root()).with_extension("tmp");
    fs::create_dir_all(&tmp).unwrap();
    assert!(l.put(entry("b", MirrorState::Current)).is_err());
    assert!(l.remove("a").is_err());
    assert!(l.set_folders(Some("x".into()), None).is_err());
    assert_eq!(l.data, before);
}

#[test]
fn stamp_changes_when_actions_log_or_nbk_changes_and_ignores_title() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "One");
    let dir = k.root.join(ID_A);
    let a = stamp_source(&dir).unwrap();
    assert_eq!((a.actions_len, a.actions_mtime_ms), (0, 0));
    assert!(a.nbk_len > 0);
    k.rename(ID_A, "Two");
    assert_eq!(a, stamp_source(&dir).unwrap());
    k.touch(ID_A);
    let b = stamp_source(&dir).unwrap();
    assert_ne!(a, b);
    assert_eq!(b.actions_len, 1);
    fs::write(dir.join("nbk"), b"SQLite format 3\0 not a notebook, longer").unwrap();
    assert_ne!(b, stamp_source(&dir).unwrap());
}

#[test]
fn scan_classifies_create_rebuild_rename_restore_and_remove() {
    let k = Kindle::new();
    for (id, t) in [
        (ID_A, "New"),
        (ID_B, "Edited"),
        (ID_C, "Renamed"),
        (ID_D, "Back"),
    ] {
        k.add_synthetic(id, t);
    }
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    // B: stamped, then edited on the Kindle.
    l.put(entry_at(&k, ID_B, "Edited", MirrorState::Current))
        .unwrap();
    k.touch(ID_B);
    // C: same stamp, old title.
    l.put(entry_at(&k, ID_C, "Old title", MirrorState::Current))
        .unwrap();
    // D: removed earlier, source is back.
    l.put(entry_at(&k, ID_D, "Back", MirrorState::Removed))
        .unwrap();
    // E: mirrored, then the notebook was deleted on the Kindle.
    k.add_synthetic(ID_E, "Gone");
    l.put(entry_at(&k, ID_E, "Gone", MirrorState::Current))
        .unwrap();
    k.remove(ID_E);

    let sources = k.sources();
    let got = scan(&sources, &l).unwrap();
    let src = |id: &str, title: &str| SourceNotebook {
        origin_id: id.into(),
        title: title.into(),
        dir: k.root.join(id),
    };
    let st = |id: &str| stamp_source(&k.root.join(id)).unwrap();
    assert_eq!(
        got.actions,
        vec![
            MirrorAction::Create {
                source: src(ID_A, "New"),
                stamp: st(ID_A)
            },
            MirrorAction::Rebuild {
                source: src(ID_B, "Edited"),
                stamp: st(ID_B)
            },
            MirrorAction::Rename {
                source: src(ID_C, "Renamed")
            },
            MirrorAction::Restore {
                source: src(ID_D, "Back"),
                stamp: st(ID_D)
            },
            MirrorAction::Remove {
                origin_id: ID_E.into()
            },
        ]
    );
    assert_eq!(got.current, 0);
    assert!(got.unsupported.is_empty());
}

#[test]
fn scan_counts_current_and_handles_absent_sources_by_state() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "Same");
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry_at(&k, ID_A, "Same", MirrorState::Current))
        .unwrap();
    l.put(entry(ID_B, MirrorState::Unsupported)).unwrap();
    l.put(entry(ID_C, MirrorState::Rebuilding)).unwrap();
    l.put(entry(ID_D, MirrorState::Removed)).unwrap();
    let mut none = entry(ID_E, MirrorState::Rebuilding);
    none.dest_notebook_id = None;
    l.put(none).unwrap();
    let got = scan(&k.sources(), &l).unwrap();
    assert_eq!(got.current, 1);
    assert_eq!(
        got.actions,
        vec![
            MirrorAction::Remove {
                origin_id: ID_B.into()
            },
            MirrorAction::Remove {
                origin_id: ID_C.into()
            },
            MirrorAction::Remove {
                origin_id: ID_E.into()
            },
        ]
    );
}

#[test]
fn scan_with_missing_catalog_and_root_is_empty_not_an_error() {
    let k = Kindle::new();
    fs::remove_dir_all(&k.root).unwrap();
    fs::remove_file(&k.catalog).unwrap();
    let s = Scribe::new();
    let l = MirrorLedger::open(s.lib.root()).unwrap();
    assert_eq!(scan(&k.sources(), &l).unwrap(), MirrorScan::default());
}

#[test]
fn scan_skips_unsupported_sources_until_their_stamp_changes() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "Odd");
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    let mut e = entry_at(&k, ID_A, "Odd", MirrorState::Unsupported);
    e.error = Some("unsupported layout".into());
    l.put(e).unwrap();
    let got = scan(&k.sources(), &l).unwrap();
    assert!(got.actions.is_empty());
    assert_eq!(
        got.unsupported,
        vec![("Odd".into(), "unsupported layout".into())]
    );
    k.touch(ID_A);
    let got = scan(&k.sources(), &l).unwrap();
    assert!(got.unsupported.is_empty());
    assert!(matches!(
        got.actions.as_slice(),
        [MirrorAction::Rebuild { .. }]
    ));
}

#[test]
fn scan_is_stat_only_and_leaves_amazon_storage_untouched() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "One");
    let before = k.fingerprint();
    let s = Scribe::new();
    let l = MirrorLedger::open(s.lib.root()).unwrap();
    scan(&k.sources(), &l).unwrap();
    assert_eq!(before, k.fingerprint());
}

#[test]
fn scan_with_a_configured_but_missing_catalog_takes_no_action() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "One");
    k.add_synthetic(ID_B, "Two");
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry_at(&k, ID_A, "One", MirrorState::Current))
        .unwrap();
    let before = l.data.clone();
    fs::remove_file(&k.catalog).unwrap();
    assert_eq!(scan(&k.sources(), &l).unwrap(), MirrorScan::default());
    assert_eq!(l.data, before);
    assert_eq!(MirrorLedger::open(s.lib.root()).unwrap().data, before);
}

#[test]
fn a_source_vanishing_between_discover_and_stamp_is_skipped() {
    // The race itself cannot be provoked deterministically; cover the classification the
    // skip branch in `scan` relies on: a vanished directory is a NotFound stat error.
    let k = Kindle::new();
    k.add_synthetic(ID_A, "One");
    let dir = k.root.join(ID_A);
    fs::remove_dir_all(&dir).unwrap();
    assert!(stamp_source(&dir).is_err());
    assert_eq!(
        try_stamp(&dir).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn rebuilding_and_stampless_unsupported_sources_that_are_present_rebuild() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "Half");
    k.add_synthetic(ID_B, "Odd");
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry_at(&k, ID_A, "Half", MirrorState::Rebuilding))
        .unwrap();
    let mut e = entry(ID_B, MirrorState::Unsupported);
    e.error = Some("x".into());
    l.put(e).unwrap();
    let got = scan(&k.sources(), &l).unwrap();
    assert!(got.unsupported.is_empty());
    assert!(matches!(
        got.actions.as_slice(),
        [MirrorAction::Rebuild { source: a, .. }, MirrorAction::Rebuild { source: b, .. }]
            if a.origin_id == ID_A && b.origin_id == ID_B
    ));
}

fn stroke(xs: &[i64], ys: &[i64]) -> nbk_reader::DecodedStroke {
    nbk_reader::DecodedStroke {
        xs: xs.to_vec(),
        ys: ys.to_vec(),
        pressure: Some(vec![5; xs.len()]),
        brush_type: Some(1),
        color: Some(0),
        thickness: Some(1.5),
        diagnostics: vec![],
    }
}

fn one_page(strokes: Vec<nbk_reader::DecodedStroke>) -> nbk_reader::DecodedNotebook {
    nbk_reader::DecodedNotebook {
        pages: vec![nbk_reader::DecodedPage {
            page_id: "p1".into(),
            missing: false,
            width: 1000,
            height: 2000,
            normalized_ppi: Default::default(),
            template_id: None,
            field_fallbacks: vec![],
            strokes,
        }],
        ..Default::default()
    }
}

#[test]
fn content_hash_is_stable_and_ignores_diagnostics_and_book_id() {
    let a = one_page(vec![stroke(&[1, 2, 3], &[4, 5, 6])]);
    let mut b = a.clone();
    b.book_id = Some("book".into());
    b.diagnostics.delta_count = 9;
    b.template_ids = vec!["t".into()];
    b.pages[0].strokes[0].diagnostics = vec!["note".into()];
    let h = content_hash(&a);
    assert_eq!(h.len(), 64);
    assert!(h
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    assert_eq!(h, content_hash(&a));
    assert_eq!(h, content_hash(&b));

    let moved = one_page(vec![stroke(&[1, 2, 4], &[4, 5, 6])]);
    assert_ne!(h, content_hash(&moved));
    // Boundary shift between xs and ys must not collide.
    assert_ne!(
        content_hash(&one_page(vec![stroke(&[1, 2], &[3, 4, 5])])),
        content_hash(&one_page(vec![stroke(&[1, 2, 3], &[4, 5])]))
    );
}

#[test]
fn decode_errors_split_into_transient_and_permanent() {
    use nbk_reader::NbkError;
    assert!(matches!(
        classify_decode_error(&NbkError::PendingActions {
            nbk_strokes: 1,
            action_stroke_inserts: 2
        }),
        DecodeFailure::Transient(_)
    ));
    for e in [
        NbkError::Io("x".into()),
        NbkError::SourceBusy("x".into()),
        NbkError::SourceChanged("x".into()),
    ] {
        assert!(matches!(
            classify_decode_error(&e),
            DecodeFailure::Transient(_)
        ));
    }
    for e in [
        NbkError::Container("x".into()),
        NbkError::Unsupported("x".into()),
    ] {
        assert!(matches!(
            classify_decode_error(&e),
            DecodeFailure::Permanent(_)
        ));
    }
}

// ------------------------------------------------------------------------------ folders and sync hooks

#[test]
fn ensure_folder_creates_once_and_reuses_even_after_rename() {
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    let mut f = crate::folders::FolderStore::open(s.lib.root());
    let live = s.lib.manifest().notebook_ids.clone();
    let a = ensure_folder(&mut l, &mut f, &live, Which::Amazon, &NoSync).unwrap();
    assert_eq!(f.title(&a), Some(AMAZON_FOLDER_TITLE));
    assert_eq!(l.data.amazon_folder.as_deref(), Some(a.as_str()));
    assert_eq!(l.data.removed_folder, None);
    assert_eq!(
        ensure_folder(&mut l, &mut f, &live, Which::Amazon, &NoSync).unwrap(),
        a
    );
    f.rename(&a, "Kindle", &live).unwrap();
    assert_eq!(
        ensure_folder(&mut l, &mut f, &live, Which::Amazon, &NoSync).unwrap(),
        a
    );
    assert_eq!(f.title(&a), Some("Kindle"));
    let r = ensure_folder(&mut l, &mut f, &live, Which::Removed, &NoSync).unwrap();
    assert_ne!(r, a);
    assert_eq!(f.title(&r), Some(REMOVED_FOLDER_TITLE));
    assert_eq!(l.data.amazon_folder.as_deref(), Some(a.as_str()));
    assert_eq!(MirrorLedger::open(s.lib.root()).unwrap().data, l.data);
}

#[test]
fn ensure_folder_recreates_when_the_user_deleted_it() {
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    let mut f = crate::folders::FolderStore::open(s.lib.root());
    let live = s.lib.manifest().notebook_ids.clone();
    let a = ensure_folder(&mut l, &mut f, &live, Which::Amazon, &NoSync).unwrap();
    f.delete(&a, &live).unwrap();
    let b = ensure_folder(&mut l, &mut f, &live, Which::Amazon, &NoSync).unwrap();
    assert_ne!(a, b);
    assert_eq!(f.title(&b), Some(AMAZON_FOLDER_TITLE));
    assert_eq!(l.data.amazon_folder.as_deref(), Some(b.as_str()));
}

fn two_page_notebook(s: &mut Scribe, id: &str) -> PathBuf {
    let g = geo();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut t = crate::nbk_import::ImportTarget {
        library: &mut s.lib,
        profile: &s.profile,
        geo: &g,
        paper: &s.paper,
        cancel: &cancel,
    };
    crate::nbk_import::build(
        &mut t,
        id,
        "Old",
        &[vec![], vec![]],
        ("m.json", b"{}"),
        &NoSync,
    )
    .unwrap();
    s.lib.notebook_dir(id)
}

#[test]
fn no_sync_delete_page_removes_the_page() {
    let mut s = Scribe::new();
    let dir = two_page_notebook(&mut s, "notebook-7");
    let mut m = crate::library::read_notebook_manifest_read_only(&dir).unwrap();
    let gone = m.page_ids[0].clone();
    NoSync
        .delete_page(&dir, &mut m, "notebook-7", &gone)
        .unwrap();
    assert_eq!(m.page_ids.len(), 1);
    let re = crate::library::read_notebook_manifest_read_only(&dir).unwrap();
    assert_eq!(re.page_ids, m.page_ids);
    assert!(!re.page_ids.contains(&gone));
}

#[test]
fn no_sync_rename_notebook_retitles_it() {
    let mut s = Scribe::new();
    two_page_notebook(&mut s, "notebook-7");
    NoSync
        .rename_notebook(&mut s.lib, "notebook-7", "New")
        .unwrap();
    assert_eq!(s.lib.title("notebook-7").unwrap(), "New");
    s.reopen();
    assert_eq!(s.lib.title("notebook-7").unwrap(), "New");
}

#[test]
fn delete_page_refuses_the_only_page_and_propagates_the_refusal() {
    let mut s = Scribe::new();
    let g = geo();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut t = crate::nbk_import::ImportTarget {
        library: &mut s.lib,
        profile: &s.profile,
        geo: &g,
        paper: &s.paper,
        cancel: &cancel,
    };
    crate::nbk_import::build(
        &mut t,
        "notebook-7",
        "One",
        &[vec![]],
        ("m.json", b"{}"),
        &NoSync,
    )
    .unwrap();
    let dir = s.lib.notebook_dir("notebook-7");
    let mut m = crate::library::read_notebook_manifest_read_only(&dir).unwrap();
    let only = m.page_ids[0].clone();
    let e = NoSync
        .delete_page(&dir, &mut m, "notebook-7", &only)
        .unwrap_err();
    assert!(e.starts_with("REFUSE PAGE DELETE"), "{e}");
    let re = crate::library::read_notebook_manifest_read_only(&dir).unwrap();
    assert_eq!(re.page_ids, vec![only]);
}

// ------------------------------------------------------------------------------ engine

use crate::library::read_notebook_manifest_read_only;
use crate::nbk_import::ImportTarget;
use crate::store::load_page_read_only;
use crate::sync_outbound::DurableWaitError;
use nbk_reader::{DecodedNotebook, NbkError};
use notebook_core::Stroke;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

type Decoded = Result<DecodedNotebook, NbkError>;
type Pages = [&'static [(i64, i64)]];

const ONE: &Pages = &[&[(10, 20), (30, 40)], &[(50, 60), (70, 80), (90, 100)]];
const TWO: &Pages = &[&[(1, 2), (3, 4)], &[(5, 6)], &[(7, 8), (9, 10)]];

/// Canvas 15624x20832 pages (the only one `convert` accepts), one stroke each.
fn decoded(pages: &[&[(i64, i64)]]) -> DecodedNotebook {
    DecodedNotebook {
        pages: pages
            .iter()
            .enumerate()
            .map(|(i, pts)| nbk_reader::DecodedPage {
                page_id: format!("p{i}"),
                missing: false,
                width: 15624,
                height: 20832,
                normalized_ppi: None,
                template_id: None,
                field_fallbacks: vec![],
                strokes: vec![nbk_reader::DecodedStroke {
                    xs: pts.iter().map(|p| p.0).collect(),
                    ys: pts.iter().map(|p| p.1).collect(),
                    pressure: Some(vec![2000; pts.len()]),
                    brush_type: Some(1),
                    color: Some(0),
                    thickness: Some(1.5),
                    diagnostics: vec![],
                }],
            })
            .collect(),
        ..Default::default()
    }
}

fn decode_from(map: HashMap<PathBuf, Decoded>) -> impl Fn(&Path) -> Decoded {
    move |p: &Path| {
        map.get(p)
            .cloned()
            .unwrap_or_else(|| Err(NbkError::Io(format!("no fixture for {}", p.display()))))
    }
}

/// Drawn content as page -> stroke -> points (what `convert` keeps).
fn expect(d: &DecodedNotebook) -> Vec<Vec<Vec<(i64, i64)>>> {
    d.pages
        .iter()
        .map(|p| {
            p.strokes
                .iter()
                .filter(|s| !s.xs.is_empty() && s.xs.len() == s.ys.len())
                .map(|s| s.xs.iter().copied().zip(s.ys.iter().copied()).collect())
                .collect()
        })
        .collect()
}

/// Records every native mutation the engine reports, delegating durable writes to `NoSync`.
#[derive(Default)]
struct Rec {
    ev: RefCell<Vec<String>>,
    flushes: Cell<usize>,
    fail_flush_at: Option<usize>,
    /// The next `delete_page` fails (a non-halt error mid-rebuild).
    fail_delete_page: Cell<bool>,
}

impl Rec {
    fn failing_at(n: usize) -> Self {
        Self {
            fail_flush_at: Some(n),
            ..Default::default()
        }
    }
    fn failing_delete_page() -> Self {
        let r = Self::default();
        r.fail_delete_page.set(true);
        r
    }
    fn push(&self, e: String) {
        self.ev.borrow_mut().push(e);
    }
    fn events(&self) -> Vec<String> {
        self.ev.borrow().clone()
    }
}

impl ImportSync for Rec {
    fn notebook_created(&self, _: &LibraryRepository, id: &str, _: &str) {
        self.push(format!("notebook_created:{id}"));
    }
    fn page_created(&self, _: &Path, _: &str, page: &str, _: &str) {
        self.push(format!("page_created:{page}"));
    }
    fn strokes_committed(&self, _: &str, page: &str, strokes: &[Stroke]) {
        self.push(format!("strokes:{page}:{}", strokes.len()));
    }
    fn flush(&self, _: &AtomicBool) -> Result<(), DurableWaitError> {
        let n = self.flushes.get() + 1;
        self.flushes.set(n);
        if self.fail_flush_at == Some(n) {
            self.push("flush_failed".into());
            return Err(DurableWaitError::Stopped);
        }
        self.push("flush".into());
        Ok(())
    }
    fn delete_notebook(&self, library: &mut LibraryRepository, id: &str) -> Result<(), String> {
        self.push(format!("delete_notebook:{id}"));
        NoSync.delete_notebook(library, id)
    }
}

impl MirrorSync for Rec {
    fn delete_page(
        &self,
        nb_dir: &Path,
        manifest: &mut PersistedNotebookManifestV1,
        notebook_id: &str,
        page_id: &str,
    ) -> Result<(), String> {
        self.push(format!("delete_page:{page_id}:{}", manifest.page_ids.len()));
        if self.fail_delete_page.replace(false) {
            return Err("boom".into());
        }
        NoSync.delete_page(nb_dir, manifest, notebook_id, page_id)
    }
    fn rename_notebook(
        &self,
        library: &mut LibraryRepository,
        notebook_id: &str,
        title: &str,
    ) -> Result<(), String> {
        self.push(format!("rename:{title}"));
        NoSync.rename_notebook(library, notebook_id, title)
    }
    fn folder_changed(&self, _: &FolderStore, change: FolderCapture) {
        self.push(match change {
            FolderCapture::Filed { folder, .. } => format!("filed:{}", folder.unwrap_or_default()),
            FolderCapture::Created(id) => format!("folder_created:{id}"),
            _ => "folder_other".into(),
        });
    }
}

/// A fake Kindle, a scratch library, its folders and the mirror ledger.
struct Rig {
    k: Kindle,
    s: Scribe,
    folders: FolderStore,
    ledger: MirrorLedger,
}

impl Rig {
    fn new() -> Self {
        let s = Scribe::new();
        Self {
            k: Kindle::new(),
            folders: FolderStore::open(s.lib.root()),
            ledger: MirrorLedger::open(s.lib.root()).unwrap(),
            s,
        }
    }

    fn with(&self, items: Vec<(&str, Decoded)>) -> impl Fn(&Path) -> Decoded + use<> {
        decode_from(
            items
                .into_iter()
                .map(|(id, d)| (self.k.root.join(id), d))
                .collect(),
        )
    }

    fn apply(
        &mut self,
        action: &MirrorAction,
        decode: &dyn Fn(&Path) -> Decoded,
        sync: &dyn MirrorSync,
    ) -> MirrorOutcome {
        let g = geo();
        let mut t = MirrorTarget {
            import: ImportTarget {
                library: &mut self.s.lib,
                profile: &self.s.profile,
                geo: &g,
                paper: &self.s.paper,
                cancel: &NO_CANCEL,
            },
            folders: &mut self.folders,
        };
        apply_one(&mut t, &mut self.ledger, action, decode, sync)
    }

    /// Scan, then apply every action in order.
    fn run(
        &mut self,
        decode: &dyn Fn(&Path) -> Decoded,
        sync: &dyn MirrorSync,
    ) -> Vec<(MirrorAction, MirrorOutcome)> {
        let actions = scan(&self.k.sources(), &self.ledger).unwrap().actions;
        actions
            .into_iter()
            .map(|a| {
                let o = self.apply(&a, decode, sync);
                (a, o)
            })
            .collect()
    }

    fn entry(&self, id: &str) -> &MirrorEntryV1 {
        &self.ledger.data.entries[id]
    }

    fn dest(&self, id: &str) -> String {
        self.entry(id).dest_notebook_id.clone().unwrap()
    }

    fn pages(&self, nb: &str) -> Vec<String> {
        read_notebook_manifest_read_only(&self.s.lib.notebook_dir(nb))
            .unwrap()
            .page_ids
    }

    fn content(&self, nb: &str) -> Vec<Vec<Vec<(i64, i64)>>> {
        let dir = self.s.lib.notebook_dir(nb);
        self.pages(nb)
            .iter()
            .map(|p| {
                load_page_read_only(&dir, p)
                    .unwrap()
                    .strokes
                    .iter()
                    .map(|s| s.points.iter().map(|q| (q.x as i64, q.y as i64)).collect())
                    .collect()
            })
            .collect()
    }

    fn live(&self) -> Vec<String> {
        self.s.lib.manifest().notebook_ids.clone()
    }

    /// Library notebooks whose mirror marker names `origin_id`.
    fn mirrors_of(&self, origin_id: &str) -> Vec<String> {
        self.live()
            .into_iter()
            .filter(|nb| {
                fs::read(self.s.lib.notebook_dir(nb).join(MIRROR_MARKER))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<MirrorOriginV1>(&b).ok())
                    .is_some_and(|m| m.origin_id == origin_id)
            })
            .collect()
    }
}

fn all_done(out: &[(MirrorAction, MirrorOutcome)]) {
    assert!(
        out.iter().all(|(_, o)| *o == MirrorOutcome::Done),
        "{out:?}"
    );
}

#[test]
fn create_builds_a_read_only_notebook_in_the_amazon_folder_with_marker_and_ledger_entry() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    let before = r.live();
    let active = r.s.lib.manifest().active_notebook_id.clone();
    let out = r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Create { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );

    let nb = r.dest(ID_A);
    assert_eq!(r.live().len(), before.len() + 1);
    assert!(!before.contains(&nb));
    assert_eq!(r.s.lib.title(&nb).unwrap(), "Alpha");
    assert_eq!(
        r.s.lib.manifest().active_notebook_id,
        active,
        "active unchanged"
    );
    assert_eq!(r.content(&nb), expect(&decoded(ONE)));

    // Read-only by convention: the provenance marker.
    let marker: MirrorOriginV1 =
        serde_json::from_slice(&fs::read(r.s.lib.notebook_dir(&nb).join(MIRROR_MARKER)).unwrap())
            .unwrap();
    assert_eq!(marker.v, 1);
    assert_eq!(marker.origin_kind, ORIGIN_KIND);
    assert_eq!(marker.origin_id, ID_A);
    assert!(marker.mirrored_unix_ms > 0);

    let amazon = r.ledger.data.amazon_folder.clone().unwrap();
    assert_eq!(r.folders.title(&amazon), Some(AMAZON_FOLDER_TITLE));
    assert_eq!(r.folders.folder_of(&nb), Some(amazon.as_str()));

    let e = r.entry(ID_A).clone();
    assert_eq!(e.state, MirrorState::Current);
    assert_eq!(e.title, "Alpha");
    assert_eq!(e.pages, 2);
    assert_eq!(e.error, None);
    assert_eq!(e.stamp, Some(stamp_source(&r.k.root.join(ID_A)).unwrap()));
    assert_eq!(e.content_sha256, Some(content_hash(&decoded(ONE))));
    assert_eq!(
        MirrorLedger::open(r.s.lib.root()).unwrap().data,
        r.ledger.data
    );

    let again = scan(&r.k.sources(), &r.ledger).unwrap();
    assert!(again.actions.is_empty());
    assert_eq!(again.current, 1);
}

#[test]
fn rebuild_with_the_same_content_hash_only_updates_the_stamp() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    let pages = r.pages(&nb);
    let old = r.entry(ID_A).clone();

    r.k.touch(ID_A);
    let rec = Rec::default();
    let out = r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &rec);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Rebuild { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );
    assert_eq!(r.pages(&nb), pages);
    assert!(
        rec.events().iter().all(|e| e == "flush"),
        "no native mutation: {:?}",
        rec.events()
    );
    let e = r.entry(ID_A).clone();
    assert_eq!(e.stamp, Some(stamp_source(&r.k.root.join(ID_A)).unwrap()));
    assert_ne!(e.stamp, old.stamp);
    assert_eq!(
        (e.state, &e.content_sha256, &e.dest_notebook_id, e.pages),
        (
            MirrorState::Current,
            &old.content_sha256,
            &old.dest_notebook_id,
            old.pages
        )
    );
    assert!(scan(&r.k.sources(), &r.ledger).unwrap().actions.is_empty());
}

#[test]
fn rebuild_creates_new_pages_before_deleting_old_ones_and_never_drops_below_one_page() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    let old = r.pages(&nb);

    // Three new pages, the middle one big enough for two stroke chunks.
    let mut next = decoded(TWO);
    next.pages[1].strokes = (0..300).map(|i| stroke(&[i, i + 1], &[i, i + 2])).collect();
    r.k.touch(ID_A);
    let rec = Rec::default();
    let out = r.run(&r.with(vec![(ID_A, Ok(next.clone()))]), &rec);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Rebuild { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );

    let ev = rec.events();
    let new = r.pages(&nb);
    assert_eq!(new.len(), 3);
    assert!(new.iter().all(|p| !old.contains(p)));
    assert_eq!(r.content(&nb), expect(&next));
    assert_eq!(r.entry(ID_A).pages, 3);
    assert_eq!(r.entry(ID_A).content_sha256, Some(content_hash(&next)));

    let pos = |pred: &dyn Fn(&str) -> bool| -> Vec<usize> {
        ev.iter()
            .enumerate()
            .filter(|(_, e)| pred(e))
            .map(|(i, _)| i)
            .collect()
    };
    let creates = pos(&|e| e.starts_with("page_created:"));
    let strokes = pos(&|e| e.starts_with("strokes:"));
    let deletes = pos(&|e| e.starts_with("delete_page:"));
    assert_eq!(creates.len(), 3, "{ev:?}");
    assert_eq!(deletes.len(), old.len(), "{ev:?}");
    let first_delete = deletes[0];
    assert!(
        creates.iter().chain(&strokes).all(|i| *i < first_delete),
        "{ev:?}"
    );
    // Each create and each stroke chunk is flushed before the next mutation.
    for i in creates.iter().chain(&strokes) {
        assert_eq!(ev[i + 1], "flush", "{ev:?}");
    }
    assert!(ev.contains(&format!("strokes:{}:256", new[1])), "{ev:?}");
    assert!(ev.contains(&format!("strokes:{}:44", new[1])), "{ev:?}");
    // Old pages deleted in their original order, never the last page.
    for (k, i) in deletes.iter().enumerate() {
        let parts: Vec<&str> = ev[*i].split(':').collect();
        assert_eq!(parts[1], old[k]);
        let len: usize = parts[2].parse().unwrap();
        assert!(len >= 2, "a delete would leave {} pages: {ev:?}", len - 1);
        assert_eq!(ev[i + 1], "flush", "{ev:?}");
    }
    assert!(
        !ev.iter().any(|e| e.starts_with("notebook_created")),
        "in place: {ev:?}"
    );
}

#[test]
fn rebuild_restores_title_and_folder() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    let amazon = r.ledger.data.amazon_folder.clone().unwrap();

    // The user retitles the mirror and drags it out of the folder.
    NoSync.rename_notebook(&mut r.s.lib, &nb, "Mine").unwrap();
    let live = r.live();
    r.folders.set_folder(&nb, None, &live).unwrap();

    r.k.touch(ID_A);
    let rec = Rec::default();
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(TWO)))]), &rec));
    assert_eq!(r.dest(ID_A), nb);
    assert_eq!(r.s.lib.title(&nb).unwrap(), "Alpha");
    assert_eq!(r.folders.folder_of(&nb), Some(amazon.as_str()));
    assert_eq!(r.content(&nb), expect(&decoded(TWO)));
    let ev = rec.events();
    assert!(ev.contains(&"rename:Alpha".to_string()), "{ev:?}");
    assert!(ev.contains(&format!("filed:{amazon}")), "{ev:?}");
}

#[test]
fn transient_decode_errors_leave_the_ledger_untouched() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    let pages = r.pages(&nb);
    let live = r.live();

    r.k.touch(ID_A);
    r.k.add_synthetic(ID_B, "Beta");
    let before = r.ledger.data.clone();
    let out = r.run(
        &r.with(vec![
            (ID_A, Err(NbkError::SourceBusy("busy".into()))),
            (
                ID_B,
                Err(NbkError::PendingActions {
                    nbk_strokes: 1,
                    action_stroke_inserts: 2,
                }),
            ),
        ]),
        &NoSync,
    );
    assert!(
        matches!(
            out.as_slice(),
            [
                (MirrorAction::Rebuild { .. }, MirrorOutcome::Skipped(_)),
                (MirrorAction::Create { .. }, MirrorOutcome::Skipped(_))
            ]
        ),
        "{out:?}"
    );
    assert_eq!(r.ledger.data, before);
    assert_eq!(MirrorLedger::open(r.s.lib.root()).unwrap().data, before);
    assert_eq!(r.pages(&nb), pages);
    assert_eq!(r.live(), live);
}

#[test]
fn permanent_decode_errors_mark_unsupported_and_keep_the_old_mirror() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    let pages = r.pages(&nb);
    let old = r.entry(ID_A).clone();

    r.k.touch(ID_A);
    r.k.add_synthetic(ID_B, "Odd canvas");
    let mut odd = decoded(ONE);
    odd.pages[0].width = 1000;
    let live = r.live();
    let out = r.run(
        &r.with(vec![
            (ID_A, Err(NbkError::Unsupported("odd layout".into()))),
            (ID_B, Ok(odd)),
        ]),
        &NoSync,
    );
    assert!(
        matches!(
            out.as_slice(),
            [
                (MirrorAction::Rebuild { .. }, MirrorOutcome::Failed(_)),
                (MirrorAction::Create { .. }, MirrorOutcome::Failed(_))
            ]
        ),
        "{out:?}"
    );

    let a = r.entry(ID_A).clone();
    assert_eq!(a.state, MirrorState::Unsupported);
    assert_eq!(a.stamp, Some(stamp_source(&r.k.root.join(ID_A)).unwrap()));
    assert!(a.error.as_deref().unwrap().contains("odd layout"));
    assert_eq!(a.dest_notebook_id, old.dest_notebook_id);
    assert_eq!(r.pages(&nb), pages, "old mirror kept as-is");
    assert_eq!(r.content(&nb), expect(&decoded(ONE)));

    let b = r.entry(ID_B).clone();
    assert_eq!(b.state, MirrorState::Unsupported);
    assert_eq!(b.dest_notebook_id, None);
    assert!(b.error.as_deref().unwrap().contains("canvas"));
    assert_eq!(r.live(), live, "nothing built");

    let again = scan(&r.k.sources(), &r.ledger).unwrap();
    assert!(again.actions.is_empty(), "{again:?}");
    assert_eq!(again.unsupported.len(), 2);
}

#[test]
fn remove_files_the_mirror_under_removed_and_restore_brings_it_back() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    let pages = r.pages(&nb);
    let amazon = r.ledger.data.amazon_folder.clone().unwrap();

    r.k.remove(ID_A);
    let out = r.run(&r.with(vec![]), &NoSync);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Remove { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );
    let removed = r.ledger.data.removed_folder.clone().unwrap();
    assert_eq!(r.folders.title(&removed), Some(REMOVED_FOLDER_TITLE));
    assert_eq!(r.folders.folder_of(&nb), Some(removed.as_str()));
    assert_eq!(r.entry(ID_A).state, MirrorState::Removed);
    assert_eq!(r.pages(&nb), pages, "never deleted");
    assert!(scan(&r.k.sources(), &r.ledger).unwrap().actions.is_empty());

    // Back with the same content: refiled, pages untouched.
    r.k.add_synthetic(ID_A, "Alpha");
    let out = r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Restore { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );
    assert_eq!(r.dest(ID_A), nb);
    assert_eq!(r.pages(&nb), pages);
    assert_eq!(r.folders.folder_of(&nb), Some(amazon.as_str()));
    assert_eq!(r.entry(ID_A).state, MirrorState::Current);
    assert_eq!(
        r.entry(ID_A).stamp,
        Some(stamp_source(&r.k.root.join(ID_A)).unwrap())
    );

    // Removed again, back with new content: same notebook, rebuilt.
    r.k.remove(ID_A);
    all_done(&r.run(&r.with(vec![]), &NoSync));
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(TWO)))]), &NoSync));
    assert_eq!(r.dest(ID_A), nb);
    assert_eq!(r.content(&nb), expect(&decoded(TWO)));
    assert_eq!(r.folders.folder_of(&nb), Some(amazon.as_str()));
    assert_eq!(r.mirrors_of(ID_A), vec![nb]);

    // A source that never built anything is simply forgotten.
    r.k.add_synthetic(ID_B, "Odd");
    let bad = |r: &Rig| {
        r.with(vec![
            (ID_A, Ok(decoded(TWO))),
            (ID_B, Err(NbkError::Container("x".into()))),
        ])
    };
    r.run(&bad(&r), &NoSync);
    assert_eq!(r.entry(ID_B).dest_notebook_id, None);
    r.k.remove(ID_B);
    let out = r.run(&bad(&r), &NoSync);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Remove { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );
    assert!(!r.ledger.data.entries.contains_key(ID_B));
}

#[test]
fn recovery_after_a_crash_mid_rebuild_removes_the_partial_and_recreates_once() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    r.k.add_synthetic(ID_B, "Beta");
    let decode =
        |r: &Rig, a: &'static Pages| r.with(vec![(ID_A, Ok(decoded(a))), (ID_B, Ok(decoded(ONE)))]);
    // A is mirrored; B's create then halts right after the notebook was made.
    let a = scan(&r.k.sources(), &r.ledger).unwrap().actions;
    assert_eq!(
        r.apply(&a[0], &decode(&r, ONE), &NoSync),
        MirrorOutcome::Done
    );
    let halt_b = Rec::failing_at(2);
    let out = r.apply(&a[1], &decode(&r, ONE), &halt_b);
    assert!(
        matches!(&out, MirrorOutcome::Halted(m) if m.starts_with("sync halted")),
        "{out:?}"
    );
    let b_partial = r.dest(ID_B);
    assert_eq!(r.entry(ID_B).state, MirrorState::Rebuilding);
    assert!(r.live().contains(&b_partial));

    // A's rebuild halts after its second new page: a half-built notebook.
    let nb = r.dest(ID_A);
    r.k.touch(ID_A);
    let halt_a = Rec::failing_at(3);
    let actions = scan(&r.k.sources(), &r.ledger).unwrap().actions;
    let out = r.apply(&actions[0], &decode(&r, TWO), &halt_a);
    assert!(matches!(&out, MirrorOutcome::Halted(_)), "{out:?}");
    assert_eq!(r.entry(ID_A).state, MirrorState::Rebuilding);
    assert_eq!(r.pages(&nb).len(), 4, "two old + two new");

    // A stale entry that never got a notebook.
    let mut c = entry(ID_C, MirrorState::Rebuilding);
    c.dest_notebook_id = None;
    r.ledger.put(c).unwrap();

    // Restart.
    let rec = Rec::default();
    assert_eq!(recover_interrupted(&mut r.s.lib, &rec), Ok(3));
    let ev = rec.events();
    assert!(ev.contains(&format!("delete_notebook:{nb}")), "{ev:?}");
    assert!(
        ev.contains(&format!("delete_notebook:{b_partial}")),
        "{ev:?}"
    );
    assert!(!r.live().contains(&nb) && !r.live().contains(&b_partial));
    r.ledger = MirrorLedger::open(r.s.lib.root()).unwrap();
    assert!(r.ledger.data.entries.is_empty());

    let out = r.run(&decode(&r, TWO), &NoSync);
    assert!(
        matches!(
            out.as_slice(),
            [
                (MirrorAction::Create { .. }, MirrorOutcome::Done),
                (MirrorAction::Create { .. }, MirrorOutcome::Done)
            ]
        ),
        "{out:?}"
    );
    assert_eq!(r.mirrors_of(ID_A), vec![r.dest(ID_A)]);
    assert_eq!(r.mirrors_of(ID_B), vec![r.dest(ID_B)]);
    assert_eq!(r.content(&r.dest(ID_A)), expect(&decoded(TWO)));
    assert_eq!(recover_interrupted(&mut r.s.lib, &NoSync), Ok(0));
}

#[test]
fn the_source_tree_is_never_written() {
    let mut r = Rig::new();
    for (id, t) in [(ID_A, "A"), (ID_B, "B"), (ID_C, "C"), (ID_D, "D")] {
        r.k.add_synthetic(id, t);
    }
    let decode = |r: &Rig, a: &'static Pages| {
        r.with(vec![
            (ID_A, Ok(decoded(a))),
            (ID_B, Ok(decoded(ONE))),
            (ID_C, Ok(decoded(TWO))),
            (ID_D, Err(NbkError::Unsupported("odd".into()))),
        ])
    };
    let mut kinds = std::collections::BTreeSet::new();
    let mut phase = |r: &mut Rig, a: &'static Pages| {
        let before = (r.k.fingerprint(), r.k.tree_stat());
        for (act, o) in r.run(&decode(r, a), &NoSync) {
            assert!(
                matches!(o, MirrorOutcome::Done | MirrorOutcome::Failed(_)),
                "{o:?}"
            );
            kinds.insert(format!("{:?}", std::mem::discriminant(&act)));
        }
        assert_eq!(
            before,
            (r.k.fingerprint(), r.k.tree_stat()),
            "Amazon state changed"
        );
    };
    phase(&mut r, ONE); // Create x3, plus a Permanent failure
    r.k.touch(ID_A);
    r.k.rename(ID_B, "B2");
    r.k.remove(ID_C);
    phase(&mut r, TWO); // Rebuild, Rename, Remove
    r.k.add_synthetic(ID_C, "C");
    phase(&mut r, TWO); // Restore
    assert_eq!(kinds.len(), 5, "every action kind ran");
}

#[test]
#[ignore = "private samples: set IGNITE_NBK_IMPORT_SAMPLES and IGNITE_NBK_IMPORT_FIXTURES; run with -- --ignored"]
fn mirrors_a_real_notebook_with_exact_geometry() {
    let mut r = Rig::new();
    let scratch = fixture_role_id("scratch");
    r.k.add(ID_A, &scratch, "Scratch");
    let before = (r.k.fingerprint(), r.k.tree_stat());
    all_done(&r.run(&kindle_decode, &NoSync));
    let nb = r.dest(ID_A);
    assert_eq!(r.s.lib.title(&nb).unwrap(), "Scratch");

    // Native pages hold exactly the decoded geometry (canonical == Amazon canvas).
    let decoded = kindle_decode(&sample(&scratch)).unwrap();
    assert_eq!(r.content(&nb), expect(&decoded));
    let dir = r.s.lib.notebook_dir(&nb);
    for (page_id, dp) in r.pages(&nb).iter().zip(&decoded.pages) {
        let page = load_page_read_only(&dir, page_id).unwrap();
        let drawn = dp.strokes.iter().filter(|s| !s.xs.is_empty());
        for (n, d) in page.strokes.iter().zip(drawn) {
            if let Some(p) = d.pressure.as_ref() {
                let p0 = p[0] as f32 / 4095.0;
                assert!((n.points[0].pressure.unwrap() - p0).abs() < 1e-6);
            }
        }
    }
    assert_eq!(r.entry(ID_A).content_sha256, Some(content_hash(&decoded)));
    assert_eq!(r.mirrors_of(ID_A), vec![nb]);
    assert_eq!(
        before,
        (r.k.fingerprint(), r.k.tree_stat()),
        "Amazon state must never change"
    );
}

#[test]
fn a_failed_mid_rebuild_partial_is_removed_before_the_entry_becomes_unsupported_or_removed() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    r.k.add_synthetic(ID_B, "Beta");
    let good =
        |r: &Rig, p: &'static Pages| r.with(vec![(ID_A, Ok(decoded(p))), (ID_B, Ok(decoded(p)))]);
    all_done(&r.run(&good(&r, ONE), &NoSync));
    r.k.touch(ID_A);
    r.k.touch(ID_B);
    let actions = scan(&r.k.sources(), &r.ledger).unwrap().actions;
    for a in &actions {
        let boom = Rec::failing_delete_page();
        let out = r.apply(a, &good(&r, TWO), &boom);
        assert!(
            matches!(&out, MirrorOutcome::Failed(m) if m.contains("boom")),
            "{out:?}"
        );
    }
    let (nb_a, nb_b) = (r.dest(ID_A), r.dest(ID_B));
    for (id, nb) in [(ID_A, &nb_a), (ID_B, &nb_b)] {
        assert_eq!(r.entry(id).state, MirrorState::Rebuilding);
        assert_eq!(r.pages(nb).len(), 5, "partial: two old + three new");
    }

    // A: the next attempt finds the source permanently undecodable.
    let actions = scan(&r.k.sources(), &r.ledger).unwrap().actions;
    let rebuild_a = actions
        .iter()
        .find(|a| matches!(a, MirrorAction::Rebuild { source, .. } if source.origin_id == ID_A))
        .unwrap();
    let rec = Rec::default();
    let bad = r.with(vec![(ID_A, Err(NbkError::Unsupported("odd".into())))]);
    assert!(matches!(
        r.apply(rebuild_a, &bad, &rec),
        MirrorOutcome::Failed(_)
    ));
    assert!(rec.events().contains(&format!("delete_notebook:{nb_a}")));
    assert!(!r.live().contains(&nb_a));
    let a = r.entry(ID_A).clone();
    assert_eq!(a.state, MirrorState::Unsupported);
    assert_eq!(a.dest_notebook_id, None);

    // B: the source is deleted on the Kindle.
    r.k.remove(ID_B);
    let rec = Rec::default();
    let out = r.apply(
        &MirrorAction::Remove {
            origin_id: ID_B.into(),
        },
        &r.with(vec![]),
        &rec,
    );
    assert_eq!(out, MirrorOutcome::Done);
    assert!(rec.events().contains(&format!("delete_notebook:{nb_b}")));
    assert!(!r.live().contains(&nb_b));
    assert!(!r.ledger.data.entries.contains_key(ID_B));
    assert_eq!(recover_interrupted(&mut r.s.lib, &NoSync), Ok(0));
}

#[test]
fn an_unreadable_marker_fails_the_action_and_never_builds_a_second_mirror() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let nb = r.dest(ID_A);
    fs::write(r.s.lib.notebook_dir(&nb).join(MIRROR_MARKER), b"{").unwrap();
    r.k.touch(ID_A);
    let before = r.ledger.data.clone();
    let live = r.live();
    let out = r.run(&r.with(vec![(ID_A, Ok(decoded(TWO)))]), &NoSync);
    assert!(
        matches!(out.as_slice(), [(MirrorAction::Rebuild { .. }, MirrorOutcome::Failed(m))] if m.contains("marker")),
        "{out:?}"
    );
    assert_eq!(r.ledger.data, before, "ledger untouched");
    assert_eq!(r.live(), live, "no second notebook");
}

#[test]
fn rebuild_after_the_user_deleted_the_mirror_builds_exactly_one_new_notebook() {
    let mut r = Rig::new();
    r.k.add_synthetic(ID_A, "Alpha");
    all_done(&r.run(&r.with(vec![(ID_A, Ok(decoded(ONE)))]), &NoSync));
    let old = r.dest(ID_A);
    NoSync.delete_notebook(&mut r.s.lib, &old).unwrap();
    r.k.touch(ID_A);
    let out = r.run(&r.with(vec![(ID_A, Ok(decoded(TWO)))]), &NoSync);
    assert!(
        matches!(
            out.as_slice(),
            [(MirrorAction::Rebuild { .. }, MirrorOutcome::Done)]
        ),
        "{out:?}"
    );
    let nb = r.dest(ID_A);
    assert_ne!(nb, old);
    assert_eq!(r.mirrors_of(ID_A), vec![nb.clone()]);
    assert_eq!(r.content(&nb), expect(&decoded(TWO)));
    assert_eq!(
        r.folders.folder_of(&nb),
        r.ledger.data.amazon_folder.as_deref()
    );
}

// ------------------------------------------------------------------------- scheduling

#[test]
fn scan_is_due_at_launch_after_an_upload_batch_and_after_fifteen_idle_minutes() {
    let t0 = Instant::now();
    let launch = MirrorClock {
        last_scan: None,
        last_batches: 0,
    };
    assert!(scan_due(&launch, t0, 0));
    let after = MirrorClock {
        last_scan: Some(t0),
        last_batches: 3,
    };
    assert!(!scan_due(&after, t0, 3));
    assert!(!scan_due(&after, t0 + Duration::from_secs(60), 3));
    assert!(scan_due(&after, t0 + Duration::from_secs(60), 4));
    assert!(!scan_due(
        &after,
        t0 + IDLE_RESCAN - Duration::from_secs(1),
        3
    ));
    assert!(scan_due(&after, t0 + IDLE_RESCAN, 3));
}

#[test]
fn a_rebuild_never_runs_while_a_page_is_open() {
    use crate::home::AppView;
    for v in [
        AppView::Notebook,
        AppView::Sync,
        AppView::Preferences,
        AppView::LinkTargetPicker,
    ] {
        assert!(!step_allowed(v, false, true));
    }
    assert!(step_allowed(AppView::Home, false, true));
    assert!(step_allowed(AppView::Tools, false, true));
    assert!(!step_allowed(AppView::Home, true, true));
    assert!(!step_allowed(AppView::Tools, false, false));
    // The scan has the same gate: never between strokes with a page open.
    for v in [
        AppView::Notebook,
        AppView::Sync,
        AppView::Preferences,
        AppView::LinkTargetPicker,
    ] {
        assert!(!scan_allowed(v, false, true));
    }
    assert!(scan_allowed(AppView::Home, false, true));
    assert!(scan_allowed(AppView::Tools, false, true));
    assert!(!scan_allowed(AppView::Home, true, true));
    assert!(!scan_allowed(AppView::Home, false, false));
}

#[test]
fn a_scan_error_never_removes_existing_mirrors() {
    let k = Kindle::new();
    k.add_synthetic(ID_A, "One");
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry_at(&k, ID_A, "One", MirrorState::Current))
        .unwrap();
    let before = l.data.clone();
    // A catalog that exists but is not a readable database: discover must fail.
    fs::write(
        &k.catalog,
        b"this is not a sqlite database at all, just garbage bytes",
    )
    .unwrap();
    let err = scan(&k.sources(), &l).expect_err("a garbage catalog must fail the scan");
    assert_eq!(l.data, before);
    l.note_scan(Some(err.clone())).unwrap();
    let reopened = MirrorLedger::open(s.lib.root()).unwrap();
    assert_eq!(reopened.data.entries, before.entries);
    assert_eq!(reopened.data.last_scan_error, Some(err));
}
