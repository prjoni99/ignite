use super::*;
use crate::folders::FolderStore;
use crate::library::{
    create_notebook_durable, delete_notebook_durable, read_notebook_manifest_read_only,
    rename_notebook_durable, CreateNotebookHooks, DeleteNotebookHooks, LibraryRepository,
    RenameNotebookHooks,
};
use crate::nbk_test_kit::{geo, Scribe};
use crate::store::{load_page_read_only, load_page_texts, page_dir};
use std::cell::Cell;

fn day(year: i32, month: u32, d: u32, weekday: u32) -> LocalDateTime {
    LocalDateTime {
        year,
        month,
        day: d,
        weekday,
        hour: 9,
        minute: 30,
    }
}

/// Creations counted per kind: (folders, months, days).
#[derive(Default)]
struct Counts {
    folders: Cell<u32>,
    months: Cell<u32>,
    days: Cell<u32>,
}

/// Run `resolve_today` with real durable creations (no sync).
fn today(
    s: &mut Scribe,
    ledger: &mut DailyLedger,
    folders: &mut FolderStore,
    now: &LocalDateTime,
    counts: &Counts,
) -> Result<(String, String, bool), String> {
    let profile = s.profile.clone();
    let paper = s.paper.clone();
    let mut create = |what: CreateWhat,
                      lib: &mut LibraryRepository,
                      folders: &mut FolderStore|
     -> Result<String, String> {
        match what {
            CreateWhat::Folder => {
                counts.folders.set(counts.folders.get() + 1);
                let live = lib.manifest().notebook_ids.clone();
                folders.create(DAILY_FOLDER_TITLE, &live)
            }
            CreateWhat::Month { title, folder_id } => {
                counts.months.set(counts.months.get() + 1);
                let c = create_notebook_durable(
                    lib,
                    &profile,
                    &geo(),
                    &paper,
                    Some(&title),
                    &CreateNotebookHooks::default(),
                )?;
                let live = lib.manifest().notebook_ids.clone();
                folders.set_folder(&c.notebook_id, Some(&folder_id), &live)?;
                Ok(c.notebook_id)
            }
            CreateWhat::Day {
                notebook_id,
                after_page,
            } => {
                counts.days.set(counts.days.get() + 1);
                let dir = lib.notebook_dir(&notebook_id);
                let mut m = read_notebook_manifest_read_only(&dir)?;
                let c = insert_day_page(lib.root(), &dir, &mut m, &after_page, &geo())?;
                lib.set_cached_page_count(&notebook_id, m.page_ids.len());
                Ok(c.inserted_page_id)
            }
        }
    };
    resolve_today(ledger, &mut s.lib, folders, now, &mut create)
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
fn first_today_creates_folder_month_and_uses_page_one() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut folders = FolderStore::open(&root);
    let mut ledger = DailyLedger::open(&root).unwrap();
    let n = Counts::default();
    let now = day(2026, 10, 25, 0);
    let (nb, page, created) = today(&mut s, &mut ledger, &mut folders, &now, &n).unwrap();
    assert!(created);
    assert_eq!((n.folders.get(), n.months.get(), n.days.get()), (1, 1, 0));
    assert_eq!(page, crate::store::DEFAULT_PAGE_ID);
    assert_eq!(page_ids(&s, &nb), vec![page.clone()]);
    assert_eq!(s.lib.title(&nb).unwrap(), "Daily 2026-10");
    assert_eq!(month_title("2026-10"), "Daily 2026-10");
    let folder = folders.folder_of(&nb).expect("filed").to_string();
    assert_eq!(folders.title(&folder), Some("Daily"));
    assert_eq!(
        stamps(&s, &nb, &page),
        vec!["Sunday 25 October 2026".to_string()]
    );
    // Durable: a reopened ledger has the same answers.
    let reopened = DailyLedger::open(&root).unwrap();
    assert_eq!(reopened.data.v, 1);
    assert_eq!(reopened.data.folder.as_deref(), Some(folder.as_str()));
    assert_eq!(reopened.data.months.get("2026-10"), Some(&nb));
    assert_eq!(
        reopened.data.days.get("2026-10-25"),
        Some(&(nb.clone(), page.clone()))
    );
}

#[test]
fn second_today_same_day_reuses_the_page() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut folders = FolderStore::open(&root);
    let mut ledger = DailyLedger::open(&root).unwrap();
    let n = Counts::default();
    let now = day(2026, 10, 25, 0);
    let first = today(&mut s, &mut ledger, &mut folders, &now, &n).unwrap();
    let mut ledger = DailyLedger::open(&root).unwrap();
    let second = today(&mut s, &mut ledger, &mut folders, &now, &n).unwrap();
    assert_eq!(
        (&second.0, &second.1, second.2),
        (&first.0, &first.1, false)
    );
    assert_eq!((n.folders.get(), n.months.get(), n.days.get()), (1, 1, 0));
    assert_eq!(page_ids(&s, &first.0).len(), 1);
    assert_eq!(stamps(&s, &first.0, &first.1).len(), 1, "stamped once");
}

#[test]
fn today_after_rollover_appends_a_new_page_once() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut folders = FolderStore::open(&root);
    let mut ledger = DailyLedger::open(&root).unwrap();
    let n = Counts::default();
    let (nb, p1, _) = today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 25, 0), &n).unwrap();
    let next = day(2026, 10, 26, 1);
    let (nb2, p2, created) = today(&mut s, &mut ledger, &mut folders, &next, &n).unwrap();
    assert!(created);
    assert_eq!(nb2, nb);
    assert_ne!(p2, p1);
    assert_eq!(page_ids(&s, &nb), vec![p1.clone(), p2.clone()]);
    assert_eq!(
        stamps(&s, &nb, &p2),
        vec!["Monday 26 October 2026".to_string()]
    );
    // Again the same day: no second page.
    let again = today(&mut s, &mut ledger, &mut folders, &next, &n).unwrap();
    assert_eq!((again.1.as_str(), again.2), (p2.as_str(), false));
    assert_eq!((n.folders.get(), n.months.get(), n.days.get()), (1, 1, 1));
    assert_eq!(page_ids(&s, &nb).len(), 2);
    assert_eq!(stamps(&s, &nb, &p2).len(), 1);
}

#[test]
fn a_day_page_is_pinned_to_the_daily_template() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut folders = FolderStore::open(&root);
    let mut ledger = DailyLedger::open(&root).unwrap();
    let n = Counts::default();
    let (nb, p1, _) = today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 25, 0), &n).unwrap();
    // A different template inserted after yesterday's page: the last page is now all black.
    let dir = s.lib.notebook_dir(&nb);
    let black = vec![0u8; load_page_read_only(&dir, &p1).unwrap().background.len()];
    let mut m = read_notebook_manifest_read_only(&dir).unwrap();
    crate::notebook::insert_page_after_with_background(
        &dir,
        &mut m,
        &p1,
        &black,
        &crate::notebook::InsertPageHooks::default(),
    )
    .unwrap();
    s.lib.set_cached_page_count(&nb, m.page_ids.len());
    let (_, p2, created) =
        today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 26, 1), &n).unwrap();
    assert!(created);
    assert_eq!(page_ids(&s, &nb).last(), Some(&p2));
    let daily = crate::templates::bitmap(
        &root,
        &crate::templates::TemplateId::Builtin { id: "daily".into() },
        &geo(),
    )
    .unwrap();
    let bg = load_page_read_only(&dir, &p2).unwrap().background;
    assert!(
        bg != black,
        "today's page does not copy the previous page's template"
    );
    assert!(bg == daily, "today's page is the daily template");
}

#[test]
fn month_rollover_creates_the_next_notebook_in_the_same_folder() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut folders = FolderStore::open(&root);
    let mut ledger = DailyLedger::open(&root).unwrap();
    let n = Counts::default();
    let (oct, _, _) = today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 31, 6), &n).unwrap();
    let (nov, page, created) =
        today(&mut s, &mut ledger, &mut folders, &day(2026, 11, 1, 0), &n).unwrap();
    assert!(created);
    assert_ne!(nov, oct);
    assert_eq!(page, crate::store::DEFAULT_PAGE_ID);
    assert_eq!(s.lib.title(&nov).unwrap(), "Daily 2026-11");
    assert_eq!(folders.folder_of(&nov), folders.folder_of(&oct));
    assert!(folders.folder_of(&nov).is_some());
    assert_eq!((n.folders.get(), n.months.get(), n.days.get()), (1, 2, 0));
    assert_eq!(
        stamps(&s, &nov, &page),
        vec!["Sunday 1 November 2026".to_string()]
    );
}

#[test]
fn ledger_re_resolves_renamed_and_recreates_deleted() {
    let mut s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let mut folders = FolderStore::open(&root);
    let mut ledger = DailyLedger::open(&root).unwrap();
    let n = Counts::default();
    let now = day(2026, 10, 25, 0);
    let (nb, page, _) = today(&mut s, &mut ledger, &mut folders, &now, &n).unwrap();
    // Renamed: still the month's notebook (id-based).
    rename_notebook_durable(
        &mut s.lib,
        &nb,
        "My October",
        &RenameNotebookHooks::default(),
    )
    .unwrap();
    let r = today(&mut s, &mut ledger, &mut folders, &now, &n).unwrap();
    assert_eq!(
        (r.0.as_str(), r.1.as_str(), r.2),
        (nb.as_str(), page.as_str(), false)
    );
    // Deleted: a new month notebook; the old day entries are dropped.
    let (_, _, _) = today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 26, 1), &n).unwrap();
    delete_notebook_durable(&mut s.lib, &nb, &DeleteNotebookHooks::default()).unwrap();
    let (nb2, p2, created) = today(&mut s, &mut ledger, &mut folders, &now, &n).unwrap();
    assert!(created);
    assert_ne!(nb2, nb);
    assert_eq!(p2, crate::store::DEFAULT_PAGE_ID);
    assert_eq!(s.lib.title(&nb2).unwrap(), "Daily 2026-10");
    let d = &DailyLedger::open(&root).unwrap().data;
    assert_eq!(d.months.get("2026-10"), Some(&nb2));
    assert_eq!(d.days.get("2026-10-25"), Some(&(nb2.clone(), p2.clone())));
    assert_eq!(d.days.get("2026-10-26"), None, "old day entries dropped");
    // A deleted day page is created again.
    let (_, p26, _) = today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 26, 1), &n).unwrap();
    let dir = s.lib.notebook_dir(&nb2);
    let mut m = read_notebook_manifest_read_only(&dir).unwrap();
    crate::notebook::delete_page_durable(
        &dir,
        &mut m,
        &p26,
        &crate::notebook::DeletePageHooks::default(),
    )
    .unwrap();
    let (_, again, created) =
        today(&mut s, &mut ledger, &mut folders, &day(2026, 10, 26, 1), &n).unwrap();
    assert!(created);
    assert_ne!(again, p26);
    assert!(page_ids(&s, &nb2).contains(&again));
    // A deleted Daily folder is recreated for the next month.
    let live = s.lib.manifest().notebook_ids.clone();
    let folder = folders.folder_of(&nb2).unwrap().to_string();
    folders.delete(&folder, &live).unwrap();
    let (dec, _, _) = today(&mut s, &mut ledger, &mut folders, &day(2026, 12, 1, 2), &n).unwrap();
    let f = folders
        .folder_of(&dec)
        .expect("filed in a new Daily folder");
    assert_ne!(f, folder);
    assert_eq!(folders.title(f), Some("Daily"));
}

#[test]
fn corrupt_daily_ledger_fails_closed() {
    let s = Scribe::new();
    let root = s.lib.root().to_path_buf();
    let path = DailyLedger::path_for(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for bad in [
        &b"not json"[..],
        br#"{"v":2,"folder":null,"months":{},"days":{}}"#,
        br#"{"v":1,"folder":null,"months":{},"days":{},"extra":1}"#,
    ] {
        std::fs::write(&path, bad).unwrap();
        let e = DailyLedger::open(&root).unwrap_err();
        assert!(e.starts_with("REFUSE DAILY:"), "{e}");
    }
    assert_eq!(path, root.join(DAILY_DIR).join(DAILY_FILE));
}
