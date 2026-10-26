//! Daily page: one notebook per month (`Daily 2026-10`) in a `Daily` folder, one page per day
//! stamped with the long date. The ledger (`<library>/.scribe-daily/daily.json`) remembers the
//! folder, each month's notebook and each day's page by local id; anything it names that no
//! longer exists is re-created. Creation itself is injected so the test kit can drive it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clock::{self, LocalDateTime};
use crate::folders::FolderStore;
use crate::library::{read_notebook_manifest_read_only, LibraryRepository};
use crate::nbk_import::write_atomic;
use crate::notebook::{
    insert_page_after_durable, insert_page_after_with_background, InsertPageCommit,
    InsertPageHooks, PersistedNotebookManifestV1,
};
use crate::templates::TemplateId;
use kindle_platform::FbGeometry;

pub const DAILY_DIR: &str = ".scribe-daily";
pub const DAILY_FILE: &str = "daily.json";
pub const DAILY_FOLDER_TITLE: &str = "Daily";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyLedgerV1 {
    pub v: u32,
    /// Local folder id.
    pub folder: Option<String>,
    /// `"2026-10"` → notebook id.
    pub months: BTreeMap<String, String>,
    /// `"2026-10-25"` → (notebook id, page id).
    pub days: BTreeMap<String, (String, String)>,
}

/// The daily ledger. A corrupt one fails closed: which notebook is which month cannot be
/// proven, and guessing would create duplicates.
#[derive(Debug)]
pub struct DailyLedger {
    path: PathBuf,
    pub data: DailyLedgerV1,
}

impl DailyLedger {
    pub fn path_for(library_root: &Path) -> PathBuf {
        library_root.join(DAILY_DIR).join(DAILY_FILE)
    }

    /// A missing ledger is empty; an unreadable one or any version other than 1 is refused.
    pub fn open(library_root: &Path) -> Result<Self, String> {
        let path = Self::path_for(library_root);
        let data = match fs::read(&path) {
            Ok(b) => {
                let d: DailyLedgerV1 = serde_json::from_slice(&b)
                    .map_err(|e| format!("REFUSE DAILY: unreadable daily ledger: {e}"))?;
                if d.v != 1 {
                    return Err(format!("REFUSE DAILY: daily ledger version {}", d.v));
                }
                d
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DailyLedgerV1 {
                v: 1,
                ..Default::default()
            },
            Err(e) => return Err(format!("REFUSE DAILY: read daily ledger: {e}")),
        };
        Ok(Self { path, data })
    }

    /// Apply `f`, persist atomically, and restore the previous in-memory state if the write
    /// fails (memory == disk).
    fn commit(&mut self, f: impl FnOnce(&mut DailyLedgerV1)) -> Result<(), String> {
        let prev = self.data.clone();
        f(&mut self.data);
        let res = serde_json::to_vec_pretty(&self.data)
            .map_err(|e| e.to_string())
            .and_then(|bytes| write_atomic(&self.path, &bytes));
        if let Err(e) = res {
            self.data = prev;
            return Err(format!("daily ledger write: {e}"));
        }
        Ok(())
    }
}

/// `"2026-10"` → `"Daily 2026-10"`.
pub fn month_title(ym: &str) -> String {
    format!("Daily {ym}")
}

/// What [`resolve_today`] needs created. The runtime's closure adds the sync captures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateWhat {
    /// An empty folder titled [`DAILY_FOLDER_TITLE`]; returns its local id.
    Folder,
    /// A notebook titled `title` with the `daily` template, filed in `folder_id`; returns its
    /// id (its first page becomes today's page).
    Month { title: String, folder_id: String },
    /// A page after `after_page` (the notebook's last page); returns the new page id.
    Day {
        notebook_id: String,
        after_page: String,
    },
}

/// The creation hook: gets the library and folders `resolve_today` holds.
pub type CreateFn<'a> =
    dyn FnMut(CreateWhat, &mut LibraryRepository, &mut FolderStore) -> Result<String, String> + 'a;

/// Today's (notebook, page, created). Re-resolves everything the ledger names against the
/// library: a month notebook that is gone is created again (its month's day entries dropped),
/// a day page that is gone is appended again, a renamed notebook is still used (id-based).
/// The ledger is written only after the durable create it records; a new page is then
/// stamped with the long date (a failed stamp is logged and never blocks).
pub fn resolve_today(
    ledger: &mut DailyLedger,
    library: &mut LibraryRepository,
    folders: &mut FolderStore,
    now: &LocalDateTime,
    create: &mut CreateFn<'_>,
) -> Result<(String, String, bool), String> {
    let ym = clock::ym(now);
    let ymd = clock::ymd(now);
    let month = ledger
        .data
        .months
        .get(&ym)
        .filter(|id| library.manifest().notebook_ids.contains(id))
        .cloned();
    let Some(notebook_id) = month else {
        let folder_id = ensure_daily_folder(ledger, library, folders, create)?;
        let notebook_id = create(
            CreateWhat::Month {
                title: month_title(&ym),
                folder_id,
            },
            library,
            folders,
        )?;
        let manifest = read_notebook_manifest_read_only(&library.notebook_dir(&notebook_id))?;
        let page_id = manifest
            .page_ids
            .first()
            .cloned()
            .ok_or("daily month notebook has no page")?;
        let prefix = format!("{ym}-");
        ledger.commit(|d| {
            d.months.insert(ym.clone(), notebook_id.clone());
            d.days.retain(|k, _| !k.starts_with(&prefix));
            d.days
                .insert(ymd.clone(), (notebook_id.clone(), page_id.clone()));
        })?;
        stamp(library, &notebook_id, &page_id, now);
        return Ok((notebook_id, page_id, true));
    };
    let manifest = read_notebook_manifest_read_only(&library.notebook_dir(&notebook_id))?;
    if let Some((nb, page)) = ledger.data.days.get(&ymd) {
        if *nb == notebook_id && manifest.page_ids.contains(page) {
            return Ok((notebook_id, page.clone(), false));
        }
    }
    let after_page = manifest
        .page_ids
        .last()
        .cloned()
        .ok_or("daily month notebook has no page")?;
    let page_id = create(
        CreateWhat::Day {
            notebook_id: notebook_id.clone(),
            after_page,
        },
        library,
        folders,
    )?;
    ledger.commit(|d| {
        d.days
            .insert(ymd.clone(), (notebook_id.clone(), page_id.clone()));
    })?;
    stamp(library, &notebook_id, &page_id, now);
    Ok((notebook_id, page_id, true))
}

/// The `Daily` folder: the recorded id while it exists (a rename keeps it), else an existing
/// folder titled exactly `Daily` (a Restore brings folders back but not this ledger), else a
/// new one. Either way the id is recorded.
fn ensure_daily_folder(
    ledger: &mut DailyLedger,
    library: &mut LibraryRepository,
    folders: &mut FolderStore,
    create: &mut CreateFn<'_>,
) -> Result<String, String> {
    if let Some(id) = &ledger.data.folder {
        if folders.title(id).is_some() {
            return Ok(id.clone());
        }
    }
    let existing = folders
        .view(&library.manifest().notebook_ids)
        .into_iter()
        .find(|f| f.title == DAILY_FOLDER_TITLE)
        .map(|f| f.id);
    let id = match existing {
        Some(id) => id,
        None => create(CreateWhat::Folder, library, folders)?,
    };
    // Accepted edge (as the mirror's): if this write fails the folder exists unrecorded; the
    // next call finds it by title.
    ledger.commit(|d| d.folder = Some(id.clone()))?;
    Ok(id)
}

fn stamp(library: &LibraryRepository, notebook_id: &str, page_id: &str, now: &LocalDateTime) {
    let dir = library.notebook_dir(notebook_id);
    match crate::text_author::stamp_page(&dir, notebook_id, page_id, &clock::long_date(now)) {
        Ok(_) => println!("daily_stamped notebook={notebook_id} page={page_id}"),
        Err(e) => eprintln!("daily_stamp_failed notebook={notebook_id} page={page_id} detail={e}"),
    }
}

/// Insert a day page after `after_page`, pinned to the `daily` template (as the month
/// notebook's first page) rather than copying the previous page's background, so a template
/// put on yesterday's page does not leak into today. If the template bitmap cannot be loaded,
/// the previous page's background is copied and `daily_template_fallback` is logged.
pub fn insert_day_page(
    library_root: &Path,
    nb_dir: &Path,
    manifest: &mut PersistedNotebookManifestV1,
    after_page: &str,
    geo: &FbGeometry,
) -> Result<InsertPageCommit, String> {
    let daily = TemplateId::Builtin { id: "daily".into() };
    match crate::templates::bitmap(library_root, &daily, geo) {
        Ok(paper) => insert_page_after_with_background(
            nb_dir,
            manifest,
            after_page,
            &paper,
            &InsertPageHooks::default(),
        ),
        Err(e) => {
            println!("daily_template_fallback after={after_page} detail={e}");
            insert_page_after_durable(nb_dir, manifest, after_page, &InsertPageHooks::default())
        }
    }
}

#[cfg(test)]
#[path = "daily_tests.rs"]
mod tests;
