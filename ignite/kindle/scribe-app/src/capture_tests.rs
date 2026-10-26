use super::*;
use crate::clock::{self, LocalDateTime};
use crate::library::{
    create_notebook_durable, delete_notebook_durable, read_notebook_manifest_read_only,
    rename_notebook_durable, CreateNotebookHooks, DeleteNotebookHooks, LibraryRepository,
    RenameNotebookHooks,
};
use crate::nbk_test_kit::{geo, Scribe};
use crate::notebook::{
    delete_page_durable, insert_page_after_durable, DeletePageHooks, InsertPageHooks,
};
use crate::store::{
    ever_written_stroke_count, load_existing_page, load_page_links, load_page_texts, page_dir,
    write_page_links, DEFAULT_PAGE_ID,
};
use std::cell::Cell;

fn at(hour: u32, minute: u32) -> LocalDateTime {
    LocalDateTime {
        year: 2026,
        month: 10,
        day: 25,
        weekday: 0,
        hour,
        minute,
    }
}

/// The runtime's capture flow with real durable writes (no sync): resolve the Inbox, choose
/// the page, insert if needed, stamp. Returns (inbox, page, created).
fn capture(
    s: &mut Scribe,
    ledger: &mut CaptureLedger,
    now: &LocalDateTime,
    creates: &Cell<u32>,
) -> (String, String, bool) {
    let profile = s.profile.clone();
    let paper = s.paper.clone();
    let mut create = |lib: &mut LibraryRepository| -> Result<String, String> {
        creates.set(creates.get() + 1);
        let c = create_notebook_durable(
            lib,
            &profile,
            &geo(),
            &paper,
            Some(INBOX_TITLE),
            &CreateNotebookHooks::default(),
        )?;
        Ok(c.notebook_id)
    };
    let (nb, created) = resolve_inbox(ledger, &mut s.lib, &mut create).unwrap();
    let dir = s.lib.notebook_dir(&nb);
    let mut m = read_notebook_manifest_read_only(&dir).unwrap();
    let pd = page_dir(&dir, &m.page_ids[0]);
    let untouched = reuse_only_page(
        &m,
        ever_written_stroke_count(&pd).unwrap(),
        load_page_links(&pd).unwrap().len(),
        load_page_texts(&pd).unwrap().len(),
    );
    let page = match next_capture_page(&m, created, untouched) {
        CapturePage::ReuseUntouched(page) => return (nb, page, created),
        CapturePage::ReuseFirst => m.page_ids[0].clone(),
        CapturePage::InsertAfter(after) => {
            insert_page_after_durable(&dir, &mut m, &after, &InsertPageHooks::default())
                .unwrap()
                .inserted_page_id
        }
    };
    crate::text_author::stamp_page(&dir, &nb, &page, &clock::stamp(now)).unwrap();
    (nb, page, created)
}

/// One durable stroke on `page` (what pen-up commits).
fn draw(s: &Scribe, nb: &str, page: &str) {
    use crate::store::PageStore;
    use notebook_core::{new_stroke_id, Point, Stroke};
    let stroke = Stroke {
        id: new_stroke_id(),
        points: [(30.0, 30.0), (300.0, 300.0)]
            .iter()
            .map(|(x, y)| Point::new(*x, *y).with_pressure(0.5))
            .collect(),
        brush: Some(crate::pressure_curve::STYLE_PRESSURE_WIDTH_V1.to_string()),
        finished: true,
    };
    let (mut store, _) = load_existing_page(&s.lib.notebook_dir(nb), page).unwrap();
    store.append_committed_stroke(&stroke).unwrap();
}

fn page_ids(s: &Scribe, nb: &str) -> Vec<String> {
    read_notebook_manifest_read_only(&s.lib.notebook_dir(nb))
        .unwrap()
        .page_ids
}

fn stamps(s: &Scribe, nb: &str, page: &str) -> Vec<String> {
    load_page_texts(&page_dir(&s.lib.notebook_dir(nb), page))
        .unwrap()
        .into_iter()
        .map(|t| t.content)
        .collect()
}

#[test]
fn first_capture_creates_inbox_and_reuses_page_one() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    assert_eq!(ledger.data, CaptureLedgerV1 { v: 1, inbox: None });
    let n = Cell::new(0);
    let before = s.lib.manifest().notebook_ids.len();
    let (nb, page, created) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    assert!(created);
    assert_eq!(n.get(), 1);
    assert_eq!(page, DEFAULT_PAGE_ID);
    assert_eq!(page_ids(&s, &nb), vec![page.clone()]);
    assert_eq!(s.lib.title(&nb).unwrap(), "Inbox");
    assert_eq!(s.lib.manifest().notebook_ids.len(), before + 1);
    assert_eq!(stamps(&s, &nb, &page), vec!["Sun 25 Oct 09:30".to_string()]);
    // Durable: a reopened ledger names the Inbox.
    let reopened = CaptureLedger::open(&root).unwrap();
    assert_eq!(reopened.data.v, 1);
    assert_eq!(reopened.data.inbox.as_deref(), Some(nb.as_str()));
    assert_eq!(
        CaptureLedger::path_for(&root),
        root.join(CAPTURE_DIR).join(CAPTURE_FILE)
    );
    assert!(is_inbox(&root, &nb));
    assert!(!is_inbox(&root, "notebook-0"));
}

#[test]
fn later_captures_append_stamped_pages() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let n = Cell::new(0);
    let (nb, p1, _) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    // Written on, so the next capture appends rather than reusing it.
    draw(&s, &nb, &p1);
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let (nb2, p2, created) = capture(&mut s, &mut ledger, &at(9, 45), &n);
    assert_eq!((nb2.as_str(), created), (nb.as_str(), false));
    let (_, p3, created) = capture(&mut s, &mut ledger, &at(14, 5), &n);
    assert!(!created);
    assert_eq!(n.get(), 1, "the Inbox is created once");
    assert_eq!(page_ids(&s, &nb), vec![p1.clone(), p2.clone(), p3.clone()]);
    assert_eq!(stamps(&s, &nb, &p1), vec!["Sun 25 Oct 09:30".to_string()]);
    assert_eq!(stamps(&s, &nb, &p2), vec!["Sun 25 Oct 09:45".to_string()]);
    assert_eq!(stamps(&s, &nb, &p3), vec!["Sun 25 Oct 14:05".to_string()]);
    // Page choice is pure: only a fresh Inbox reuses its first page.
    let m = read_notebook_manifest_read_only(&s.lib.notebook_dir(&nb)).unwrap();
    assert_eq!(next_capture_page(&m, true, false), CapturePage::ReuseFirst);
    assert_eq!(
        next_capture_page(&m, false, false),
        CapturePage::InsertAfter(p3.clone())
    );
    // Several pages: never "the only page", whatever the counts.
    assert!(!reuse_only_page(&m, 0, 0, 1));
    assert_eq!(
        next_capture_page(&m, false, true),
        CapturePage::InsertAfter(p3)
    );
}

#[test]
fn ledger_re_resolves_renamed_and_recreates_deleted() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let n = Cell::new(0);
    let (nb, _, _) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    // Renamed: still the Inbox (id-based).
    rename_notebook_durable(&mut s.lib, &nb, "Scratch", &RenameNotebookHooks::default()).unwrap();
    let (again, _, created) = capture(&mut s, &mut ledger, &at(9, 31), &n);
    assert_eq!((again.as_str(), created), (nb.as_str(), false));
    assert_eq!(n.get(), 1);
    // Deleted: a new Inbox, recorded.
    delete_notebook_durable(&mut s.lib, &nb, &DeleteNotebookHooks::default()).unwrap();
    let (nb2, p, created) = capture(&mut s, &mut ledger, &at(9, 32), &n);
    assert!(created);
    assert_ne!(nb2, nb);
    assert_eq!(p, DEFAULT_PAGE_ID);
    assert_eq!(n.get(), 2);
    assert_eq!(s.lib.title(&nb2).unwrap(), "Inbox");
    assert_eq!(
        CaptureLedger::open(&root).unwrap().data.inbox.as_deref(),
        Some(nb2.as_str())
    );
    // A failed create records nothing.
    delete_notebook_durable(&mut s.lib, &nb2, &DeleteNotebookHooks::default()).unwrap();
    let mut fail = |_: &mut LibraryRepository| -> Result<String, String> { Err("boom".into()) };
    assert_eq!(
        resolve_inbox(&mut ledger, &mut s.lib, &mut fail),
        Err("boom".to_string())
    );
    assert_eq!(
        CaptureLedger::open(&root).unwrap().data.inbox.as_deref(),
        Some(nb2.as_str())
    );
}

#[test]
fn corrupt_capture_ledger_fails_closed() {
    let s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let path = CaptureLedger::path_for(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for bad in [
        &b"not json"[..],
        br#"{"v":2,"inbox":null}"#,
        br#"{"v":1,"inbox":null,"extra":1}"#,
    ] {
        std::fs::write(&path, bad).unwrap();
        let e = CaptureLedger::open(&root).unwrap_err();
        assert!(e.starts_with("REFUSE CAPTURE:"), "{e}");
        assert!(!is_inbox(&root, "notebook-0"));
    }
}

/// The runtime's discard gate, read from disk: (ever strokes, pages, links, texts).
fn discardable(s: &Scribe, nb: &str, page: &str) -> bool {
    let dir = s.lib.notebook_dir(nb);
    let pd = page_dir(&dir, page);
    // No test page here ever received a stroke; the runtime passes the store's count.
    let strokes = crate::store::load_page_read_only(&dir, page)
        .unwrap()
        .strokes
        .len();
    should_discard_on_leave(
        strokes,
        page_ids(s, nb).len(),
        load_page_links(&pd).unwrap().len(),
        load_page_texts(&pd).unwrap().len(),
    )
}

#[test]
fn empty_capture_is_discarded_on_leave_but_the_only_page_stays() {
    // (ever strokes, pages, links, texts) → discard?
    for (strokes, pages, links, texts, discard) in [
        (0, 2, 0, 1, true),
        (0, 9, 0, 0, true),
        (0, 1, 0, 1, false),
        (0, 0, 0, 0, false),
        (1, 2, 0, 1, false),
        (5, 1, 0, 1, false),
        (0, 2, 1, 1, false),
        (0, 2, 0, 2, false),
    ] {
        assert_eq!(
            should_discard_on_leave(strokes, pages, links, texts),
            discard,
            "strokes={strokes} pages={pages} links={links} texts={texts}"
        );
    }
    // On disk the only page can never be deleted, whatever the rule says.
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let n = Cell::new(0);
    let (nb, page, _) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    assert!(!discardable(&s, &nb, &page), "only page");
    let dir = s.lib.notebook_dir(&nb);
    let mut m = read_notebook_manifest_read_only(&dir).unwrap();
    let e = delete_page_durable(&dir, &mut m, &page, &DeletePageHooks::default()).unwrap_err();
    assert!(e.starts_with("REFUSE PAGE DELETE"), "{e}");
    assert_eq!(page_ids(&s, &nb), vec![page.clone()]);
    // With a second capture the untouched one may go (page one is written on, so the
    // second capture appends instead of reusing it).
    draw(&s, &nb, &page);
    let (_, p2, _) = capture(&mut s, &mut ledger, &at(9, 31), &n);
    assert!(discardable(&s, &nb, &p2));
    let mut m = read_notebook_manifest_read_only(&dir).unwrap();
    delete_page_durable(&dir, &mut m, &p2, &DeletePageHooks::default()).unwrap();
    assert_eq!(page_ids(&s, &nb), vec![page]);
}

#[test]
fn a_capture_with_a_link_or_more_text_is_kept() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let n = Cell::new(0);
    let (nb, first, _) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    draw(&s, &nb, &first);
    let dir = s.lib.notebook_dir(&nb);
    // A second text (e.g. synced in) keeps the page.
    let (_, texted, _) = capture(&mut s, &mut ledger, &at(9, 31), &n);
    assert!(discardable(&s, &nb, &texted));
    crate::text_author::stamp_page(&dir, &nb, &texted, "more").unwrap();
    assert!(!discardable(&s, &nb, &texted));
    // A link keeps the page.
    let (_, linked, _) = capture(&mut s, &mut ledger, &at(9, 32), &n);
    assert!(discardable(&s, &nb, &linked));
    use sync_engine::testkit::{lk, nb as snb, pg};
    let link = sync_protocol::SyncPageLink {
        link_id: lk(1),
        source_notebook_id: snb(1),
        source_page_id: pg(1),
        source_x: 100.0,
        source_y: 200.0,
        target_notebook_id: snb(1),
        target_page_id: pg(2),
    };
    write_page_links(&page_dir(&dir, &linked), &[link]).unwrap();
    assert!(!discardable(&s, &nb, &linked));
    // The gate is what the runtime consults before deleting: both pages stay.
    let ids = page_ids(&s, &nb);
    assert!(ids.contains(&texted) && ids.contains(&linked));
}

#[test]
fn leaving_clears_the_capture_page_and_matches_only_it() {
    let opened = || Some(("notebook-2".to_string(), "page-3".to_string()));
    let mut slot = opened();
    assert!(leaving(&mut slot, Some(("notebook-2", "page-3"))));
    assert_eq!(slot, None);
    // Once: a second leave of the same page discards nothing.
    assert!(!leaving(&mut slot, Some(("notebook-2", "page-3"))));
    for current in [
        Some(("notebook-2", "page-4")),
        Some(("notebook-1", "page-3")),
        None,
    ] {
        let mut slot = opened();
        assert!(!leaving(&mut slot, current), "{current:?}");
        assert_eq!(slot, None, "cleared on every way out");
    }
}

#[test]
fn capture_command_follows_the_inbox_flag() {
    assert_eq!(capture_command_for(false), UiCommand::Capture);
    assert_eq!(capture_command_for(true), UiCommand::CaptureReturn);
}

#[test]
fn an_untouched_only_page_is_reused_by_the_next_capture() {
    // Abandoned first capture (the only page stays on leave): the second capture reuses it.
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let n = Cell::new(0);
    let (nb, p1, created) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    assert!(created);
    let (nb2, p2, created) = capture(&mut s, &mut ledger, &at(9, 45), &n);
    assert_eq!(
        (nb2.as_str(), p2.as_str(), created),
        (nb.as_str(), p1.as_str(), false)
    );
    assert_eq!(page_ids(&s, &nb), vec![p1.clone()], "one page");
    // The first stamp is kept and no second one is written.
    assert_eq!(stamps(&s, &nb, &p1), vec!["Sun 25 Oct 09:30".to_string()]);
    // A third untouched capture still reuses it.
    let (_, p3, _) = capture(&mut s, &mut ledger, &at(10, 0), &n);
    assert_eq!(p3, p1);
    assert_eq!(stamps(&s, &nb, &p1).len(), 1);

    // A first capture with a stroke: the second appends.
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut ledger = CaptureLedger::open(&root).unwrap();
    let (nb, p1, _) = capture(&mut s, &mut ledger, &at(9, 30), &n);
    draw(&s, &nb, &p1);
    let (_, p2, created) = capture(&mut s, &mut ledger, &at(9, 45), &n);
    assert!(!created);
    assert_ne!(p2, p1);
    assert_eq!(page_ids(&s, &nb), vec![p1.clone(), p2.clone()], "two pages");
    assert_eq!(stamps(&s, &nb, &p2), vec!["Sun 25 Oct 09:45".to_string()]);

    // The pure gate: the only page, with no ever-written stroke, no link, at most one text.
    let m = read_notebook_manifest_read_only(&s.lib.notebook_dir(&nb)).unwrap();
    let mut one = m.clone();
    one.page_ids.truncate(1);
    for (strokes, links, texts, reuse) in [
        (0, 0, 0, true),
        (0, 0, 1, true),
        (1, 0, 1, false),
        (0, 1, 1, false),
        (0, 0, 2, false),
    ] {
        assert_eq!(
            reuse_only_page(&one, strokes, links, texts),
            reuse,
            "strokes={strokes} links={links} texts={texts}"
        );
    }
    assert_eq!(
        next_capture_page(&one, false, true),
        CapturePage::ReuseUntouched(one.page_ids[0].clone())
    );
    assert_eq!(next_capture_page(&one, true, true), CapturePage::ReuseFirst);
}
