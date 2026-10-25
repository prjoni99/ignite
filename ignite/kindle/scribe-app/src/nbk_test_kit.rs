//! Shared test kit for the NBK import and the Amazon mirror: a synthetic Kindle
//! (`.notebooks` root plus a `ksdk.content.db` catalog), a scratch Scribe library, and the
//! private-sample locators.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use kindle_platform::FbGeometry;

use crate::ink::InkRequest;
use crate::library::{open_or_create_library, LibraryRepository};
use crate::nbk_import::ImportSources;
use crate::profile::RenderProfile;
use crate::ui_gfx::paper_background;

static N: AtomicU64 = AtomicU64::new(0);
pub(crate) static NO_CANCEL: AtomicBool = AtomicBool::new(false);

pub(crate) fn tmp(label: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "scribe-nbk-import-{label}-{n}-{}",
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

pub(crate) fn geo() -> FbGeometry {
    FbGeometry {
        xres: 200,
        yres: 160,
        xres_virtual: 208,
        yres_virtual: 160,
        xoffset: 0,
        yoffset: 0,
        bits_per_pixel: 8,
        line_length: 208,
        smem_len: 208 * 160,
    }
}

pub(crate) fn samples_root() -> PathBuf {
    // Private notebook samples must live outside the public Ignite tree.
    // Example: IGNITE_NBK_IMPORT_SAMPLES=C:\path\to\samples
    let raw = std::env::var("IGNITE_NBK_IMPORT_SAMPLES").unwrap_or_default();
    let raw = raw.trim();
    assert!(
        !raw.is_empty(),
        "IGNITE_NBK_IMPORT_SAMPLES must be set to an absolute samples/ directory \
         (feature nbk-import-samples). Refusing to search the Ignite tree."
    );
    let root = PathBuf::from(raw);
    assert!(
        root.is_absolute(),
        "IGNITE_NBK_IMPORT_SAMPLES must be absolute, got {}",
        root.display()
    );
    root
}

pub(crate) fn sample(uuid: &str) -> PathBuf {
    samples_root().join(uuid)
}

/// Sample id of fixture `role` from the private manifest at `IGNITE_NBK_IMPORT_FIXTURES`
/// (`{"version":1,"roles":{"<role>":{"id":..}}}`; the NBK import tests validate it fully).
pub(crate) fn fixture_role_id(role: &str) -> String {
    let raw = std::env::var("IGNITE_NBK_IMPORT_FIXTURES").unwrap_or_default();
    let path = PathBuf::from(raw.trim());
    assert!(
        path.is_absolute(),
        "IGNITE_NBK_IMPORT_FIXTURES must be an absolute fixture manifest path"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).expect("fixture manifest JSON");
    v["roles"][role]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture manifest has no role {role}"))
        .trim()
        .to_string()
}

/// Fake Kindle: `.notebooks/<id>/` copies + catalog.
pub(crate) struct Kindle {
    pub(crate) root: PathBuf,
    pub(crate) catalog: PathBuf,
}

impl Kindle {
    pub(crate) fn new() -> Self {
        let base = tmp("kindle");
        let k = Kindle {
            root: base.join(".notebooks"),
            catalog: base.join("ksdk.content.db"),
        };
        fs::create_dir_all(&k.root).unwrap();
        let c = rusqlite::Connection::open(&k.catalog).unwrap();
        c.execute_batch(
            "CREATE TABLE Nodes
            (
                uuid TEXT NOT NULL PRIMARY KEY,
                title_collation    TEXT,
                author_collation   TEXT,
                node_type          TEXT,
                download_state     INTEGER DEFAULT 0,
                search_metadata    TEXT,
                last_access_time   INTEGER DEFAULT 0,
                last_modified_time INTEGER DEFAULT 0,
                creation_time      INTEGER DEFAULT 0,
                additional_data    TEXT
            );",
        )
        .unwrap();
        k
    }

    pub(crate) fn sources(&self) -> ImportSources {
        ImportSources {
            notebooks_root: self.root.clone(),
            catalog: Some(self.catalog.clone()),
        }
    }

    /// Put `from`'s notebook files under id `id` with catalog title `title`.
    pub(crate) fn add(&self, id: &str, from: &str, title: &str) {
        self.set_content(id, from);
        let c = rusqlite::Connection::open(&self.catalog).unwrap();
        c.execute(
            "INSERT OR REPLACE INTO Nodes (uuid, title_collation, node_type, additional_data)
             VALUES (?1, ?2, 'STANDALONE_NOTEBOOK', ?3)",
            rusqlite::params![
                id,
                title,
                format!(r#"{{"title":"{title}","notebook_template":"plain","delete_state":0}}"#)
            ],
        )
        .unwrap();
    }

    /// A source that needs no private sample: a stub `nbk` that is never decoded, plus the
    /// catalog row (same SQL as [`Kindle::add`]).
    pub(crate) fn add_synthetic(&self, id: &str, title: &str) {
        let dir = self.root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("nbk"), b"SQLite format 3\0 not a notebook").unwrap();
        let c = rusqlite::Connection::open(&self.catalog).unwrap();
        c.execute(
            "INSERT OR REPLACE INTO Nodes (uuid, title_collation, node_type, additional_data)
             VALUES (?1, ?2, 'STANDALONE_NOTEBOOK', ?3)",
            rusqlite::params![
                id,
                title,
                format!(r#"{{"title":"{title}","notebook_template":"plain","delete_state":0}}"#)
            ],
        )
        .unwrap();
    }

    /// Replace a source's content (the Amazon notebook was edited on the Kindle).
    pub(crate) fn set_content(&self, id: &str, from: &str) {
        let dir = self.root.join(id);
        fs::create_dir_all(&dir).unwrap();
        for f in ["nbk", "actions.log", "nbk-journal"] {
            let src = sample(from).join(f);
            if src.exists() {
                fs::copy(src, dir.join(f)).unwrap();
            }
        }
    }

    pub(crate) fn rename(&self, id: &str, title: &str) {
        let c = rusqlite::Connection::open(&self.catalog).unwrap();
        c.execute(
            "UPDATE Nodes SET title_collation = ?2 WHERE uuid = ?1",
            rusqlite::params![id, title],
        )
        .unwrap();
    }

    /// Drop a source notebook: its catalog row and its directory.
    pub(crate) fn remove(&self, id: &str) {
        let c = rusqlite::Connection::open(&self.catalog).unwrap();
        c.execute("DELETE FROM Nodes WHERE uuid = ?1", rusqlite::params![id])
            .unwrap();
        fs::remove_dir_all(self.root.join(id)).unwrap();
    }

    /// Append one byte to the source's `actions.log` so its stamp changes.
    pub(crate) fn touch(&self, id: &str) {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(id).join("actions.log"))
            .unwrap();
        f.write_all(b"\n").unwrap();
    }

    /// Every file's bytes (Amazon state must never change because of Scribe).
    pub(crate) fn fingerprint(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        out.insert("catalog".into(), fs::read(&self.catalog).unwrap());
        for d in fs::read_dir(&self.root).unwrap() {
            let d = d.unwrap();
            for f in fs::read_dir(d.path()).unwrap() {
                let f = f.unwrap();
                out.insert(
                    format!(
                        "{}/{}",
                        d.file_name().to_string_lossy(),
                        f.file_name().to_string_lossy()
                    ),
                    fs::read(f.path()).unwrap(),
                );
            }
        }
        out
    }

    /// Path, length and mtime of everything under the fake Kindle (both the notebooks root
    /// and the catalog), directories included, so even a stray journal file shows up.
    pub(crate) fn tree_stat(&self) -> BTreeMap<PathBuf, (u64, SystemTime)> {
        fn walk(p: &std::path::Path, out: &mut BTreeMap<PathBuf, (u64, SystemTime)>) {
            let m = fs::symlink_metadata(p).unwrap();
            out.insert(p.to_path_buf(), (m.len(), m.modified().unwrap()));
            if m.is_dir() {
                for e in fs::read_dir(p).unwrap() {
                    walk(&e.unwrap().path(), out);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(self.root.parent().unwrap(), &mut out);
        out
    }
}

pub(crate) struct Scribe {
    pub(crate) lib: LibraryRepository,
    pub(crate) profile: RenderProfile,
    pub(crate) paper: Vec<u8>,
}

impl Scribe {
    pub(crate) fn new() -> Self {
        let g = geo();
        let paper = paper_background(&g);
        let lib = open_or_create_library(&tmp("library"), InkRequest::Default, &g, &paper)
            .unwrap()
            .repository;
        Self {
            lib,
            profile: RenderProfile::sealed_v2_pressure(0x00).unwrap(),
            paper,
        }
    }

    pub(crate) fn reopen(&mut self) {
        self.lib = crate::library::open_library_read_only(self.lib.root()).unwrap();
    }
}
