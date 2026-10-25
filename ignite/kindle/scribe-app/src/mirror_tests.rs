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

fn decoded(strokes: Vec<nbk_reader::DecodedStroke>) -> nbk_reader::DecodedNotebook {
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
    let a = decoded(vec![stroke(&[1, 2, 3], &[4, 5, 6])]);
    let mut b = a.clone();
    b.book_id = Some("book".into());
    b.diagnostics.delta_count = 9;
    b.template_ids = vec!["t".into()];
    b.pages[0].strokes[0].diagnostics = vec!["note".into()];
    let h = content_hash(&a);
    assert_eq!(h.len(), 64);
    assert!(h.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    assert_eq!(h, content_hash(&a));
    assert_eq!(h, content_hash(&b));

    let moved = decoded(vec![stroke(&[1, 2, 4], &[4, 5, 6])]);
    assert_ne!(h, content_hash(&moved));
    // Boundary shift between xs and ys must not collide.
    assert_ne!(
        content_hash(&decoded(vec![stroke(&[1, 2], &[3, 4, 5])])),
        content_hash(&decoded(vec![stroke(&[1, 2, 3], &[4, 5])]))
    );
}

#[test]
fn decode_errors_split_into_transient_and_permanent() {
    use nbk_reader::NbkError;
    assert!(matches!(classify_decode_error(&NbkError::PendingActions { nbk_strokes: 1, action_stroke_inserts: 2 }), DecodeFailure::Transient(_)));
    for e in [NbkError::Io("x".into()), NbkError::SourceBusy("x".into()), NbkError::SourceChanged("x".into())] {
        assert!(matches!(classify_decode_error(&e), DecodeFailure::Transient(_)));
    }
    for e in [NbkError::Container("x".into()), NbkError::Unsupported("x".into())] {
        assert!(matches!(classify_decode_error(&e), DecodeFailure::Permanent(_)));
    }
}
