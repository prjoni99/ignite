# Amazon Mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stock Kindle Scribe notebooks are continuously mirrored, read-only, into an `Amazon` folder of the device's Ignite library, so they reach the sync server and browser without the owner touching the device.

**Architecture:** A new `mirror` module in `scribe-app` sits beside `nbk_import` and reuses its decoder, stroke conversion and build path. A mirror ledger (`<data-dir>/.scribe-mirror/ledger.json`) records, per Amazon notebook UUID, the source stamp and content hash last mirrored. A cheap *scan* (stat only) runs at launch, after a sync upload batch, and on a 15-minute idle timer; a *rebuild* (decode, emit ordinary sync operations) runs one notebook per loop iteration at the existing structural safe point and only while the Home or Tools view is showing. Read-only is a convention: a `mirror-origin.json` marker on the Kindle, and "notebook is in a folder titled `Amazon`" in the browser.

**Tech Stack:** Rust (scribe-app, nbk-reader, sync-protocol), Vite/React/TypeScript (ignite-web), vitest, cargo test.

**Spec:** `docs/superpowers/specs/2026-10-25-scribe-liberation-design.md`, section "1. Amazon mirror" plus "Constraints from the codebase" and "Cross-cutting".

## Global Constraints

- Amazon storage (`/mnt/us/.notebooks`, `/var/local/ksdk.content.db`) is never opened for writing, locked, journaled or renamed. Only `nbk_reader::read_source` reads it.
- Identity is the catalog UUID (`SourceNotebook.origin_id`), never title, size or mtime.
- Rebuilds emit ordinary `SyncOperationV1` operations; order is create new pages, add strokes, then delete old pages. A notebook always keeps at least one page.
- Stroke batches flush through `ImportSync::flush` before the next batch (journal backpressure), exactly as `nbk_import::build` does.
- Rebuilds run only at the structural safe point (`structure_eligible && pending_ui.is_none() && !ui.modal_open()`) and only when `ui.view` is `AppView::Home` or `AppView::Tools`. Never on the pen path. No new thread.
- Mirror is on by default when sync is linked; it never runs while sync is off (`sync_outbound::with_global` returns `None`) or faulted.
- No protocol change, no version bump of `SyncOperationV1`, `ReceiptsV1`, `FoldersFileV1` or `preferences.json` (`v` stays 1; new fields use serde defaults).
- Ledger writes are atomic (temp file + rename + dir fsync), same as `nbk_import::write_atomic`.
- Conventional commits; `cargo test -p scribe-app --lib --locked` and `npm test` in `ignite/web` pass at every commit.

## Review Focus

1. **Amazon notebook deleted on the device while its mirror is open on the Kindle.** Expected: the mirror notebook is filed under `Removed from Amazon` at the next safe point after the user leaves the page; nothing happens while the page is open. Test added to Task 6 (`a_rebuild_never_runs_while_a_page_is_open`).
2. **Source changes between stamp and read** (Amazon writing while we scan). Expected: `read_source` returns `SourceChanged`/`SourceBusy`; the ledger is untouched and the next scan retries. Test in Task 5 (`transient_decode_errors_leave_the_ledger_untouched`).
3. **Crash mid-rebuild.** Expected: at next launch the half-rebuilt mirror notebook is removed through the synced delete path and recreated by the next scan; never two mirror notebooks for one UUID. Test in Task 5 (`recovery_after_a_crash_mid_rebuild_removes_the_partial_and_recreates_once`).
4. **User renames or moves a mirror notebook on the Kindle or in the browser.** Expected: the next rebuild restores the catalog title and the `Amazon` folder; the user's rename is lost, by design, and a browser user never gets an ink tool on it. Tests in Task 5 (`rebuild_restores_title_and_folder`) and Task 8 (`isAmazonMirror`).
5. **Catalog unreadable or `.notebooks` missing** (non-Kindle dev host, or firmware change). Expected: scan reports an error once in Tools, nothing is created or removed, mirrors already present stay. Test in Task 2 (`scan_with_missing_catalog_and_root_is_empty_not_an_error`) and Task 6 (`a_scan_error_never_removes_existing_mirrors`).

---

## File structure

- Create `ignite/kindle/scribe-app/src/mirror.rs`: ledger, stamp, scan, content hash, folder helpers, rebuild engine, startup recovery. One module; it is the mirror's single responsibility and mirrors `nbk_import.rs` in shape.
- Create `ignite/kindle/scribe-app/src/mirror_tests.rs` (`#[path]`-included like `nbk_import_tests.rs`), reusing that file's `Kindle` and `Scribe` kits by moving them into a shared `nbk_test_kit.rs` (Task 1).
- Modify `ignite/kindle/scribe-app/src/nbk_import.rs`: make `convert`, `build`, `native_stroke`, `write_atomic` and `STROKES_PER_FSYNC` `pub(crate)`; `build` takes the origin marker as a parameter.
- Modify `ignite/kindle/scribe-app/src/runtime.rs`: `MirrorRun`, `mirror_step`, triggers at the safe point, startup recovery call, `FolderCapture`/`capture_folder_change` become `pub(crate)`, read-only pen gate, Home "Copy to Ignite".
- Modify `ignite/kindle/scribe-app/src/prefs.rs`: `mirror_amazon: bool` (default true).
- Modify `ignite/kindle/scribe-app/src/tools_ui.rs`: mirror status section.
- Modify `ignite/kindle/scribe-app/src/lib.rs`: `pub mod mirror;`.
- Create `ignite/web/src/model/mirror.ts` and `ignite/web/tests/mirror.test.ts`; modify `ignite/web/src/components/Viewer.tsx`.
- Modify `docs/architecture.md`, `docs/troubleshooting.md`.

---

### Task 1: Shared test kit and `nbk_import` visibility

**Files:**
- Create: `ignite/kindle/scribe-app/src/nbk_test_kit.rs`
- Modify: `ignite/kindle/scribe-app/src/nbk_import_tests.rs` (move `tmp`, `geo`, `Kindle`, `Scribe`, `NO_CANCEL`, `samples_root`, `sample` into the kit; keep the tests)
- Modify: `ignite/kindle/scribe-app/src/nbk_import.rs`

**Interfaces:**
- Produces: `pub(crate) mod nbk_test_kit` (cfg(test)) exporting `Kindle { root, catalog }` with `new()`, `sources()`, `add(id, from, title)`, `set_content(id, from)`, `remove(id)` (new: deletes the catalog row and directory) and `touch(id)` (new: appends one byte to `actions.log` so the stamp changes); `Scribe { lib, profile, paper }` with `new()`, `reopen()`; `fn tmp(label) -> PathBuf`; `fn geo() -> FbGeometry`; `static NO_CANCEL: AtomicBool`.
- Produces in `nbk_import`: `pub(crate) fn convert(nb: &DecodedNotebook) -> Result<(Vec<Vec<Stroke>>, usize), String>`; `pub(crate) fn build(t: &mut ImportTarget<'_>, dest: &str, title: &str, pages: &[Vec<Stroke>], marker: (&str, &[u8]), sync: &dyn ImportSync) -> Result<(), String>` (marker file name and bytes written into the notebook dir instead of the hard-coded `import-origin.json`); `pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String>`; `pub(crate) const STROKES_PER_FSYNC`.

- [ ] **Step 1: Move the kits**

Create `nbk_test_kit.rs` with the items listed above, `#[cfg(test)] pub(crate) mod nbk_test_kit;` in `lib.rs`, and change `nbk_import_tests.rs` to `use crate::nbk_test_kit::*;`. Add `Kindle::remove` and `Kindle::touch`.

- [ ] **Step 2: Run the existing import tests (non-ignored) to prove nothing moved**

Run: `cargo test -p scribe-app --lib --locked nbk_import`
Expected: all non-ignored tests PASS, same count as before the move.

- [ ] **Step 3: Widen `nbk_import` visibility and thread the marker parameter**

`import_one_admitted` calls `build(t, &dest, &src.title, &pages, (ORIGIN_MARKER, &origin_bytes), sync)` where `origin_bytes` is the pretty JSON of `ImportOriginV1` it already builds.

- [ ] **Step 4: Run the import tests again**

Run: `cargo test -p scribe-app --lib --locked nbk_import`
Expected: PASS, same count.

- [ ] **Step 5: Commit**

```bash
git add ignite/kindle/scribe-app/src/nbk_test_kit.rs ignite/kindle/scribe-app/src/nbk_import_tests.rs ignite/kindle/scribe-app/src/nbk_import.rs ignite/kindle/scribe-app/src/lib.rs
git commit -m "refactor(scribe-app): share the nbk test kit and open build/convert to the mirror"
```

---

### Task 2: Mirror ledger, source stamp, and scan

**Files:**
- Create: `ignite/kindle/scribe-app/src/mirror.rs`
- Create: `ignite/kindle/scribe-app/src/mirror_tests.rs`
- Modify: `ignite/kindle/scribe-app/src/lib.rs` (`pub mod mirror;`)

**Interfaces:**
- Produces:

```rust
pub const MIRROR_DIR: &str = ".scribe-mirror";
pub const LEDGER_FILE: &str = "ledger.json";
pub const MIRROR_MARKER: &str = "mirror-origin.json";
pub const AMAZON_FOLDER_TITLE: &str = "Amazon";
pub const REMOVED_FOLDER_TITLE: &str = "Removed from Amazon";
pub const ORIGIN_KIND: &str = "amazon_nbk";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStamp { pub nbk_len: u64, pub nbk_mtime_ms: u64, pub actions_len: u64, pub actions_mtime_ms: u64 }
pub fn stamp_source(dir: &Path) -> Result<SourceStamp, String>;   // metadata only; absent actions.log = zeros

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorState { Current, Rebuilding, Removed, Unsupported }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorEntryV1 {
    pub origin_id: String, pub title: String, pub state: MirrorState,
    pub dest_notebook_id: String, pub stamp: Option<SourceStamp>, pub content_sha256: Option<String>,
    pub pages: usize, pub error: Option<String>, pub updated_unix_ms: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorLedgerV1 {
    pub v: u32,
    pub entries: BTreeMap<String, MirrorEntryV1>,   // key = origin_id
    pub amazon_folder: Option<String>,              // local folder id
    pub removed_folder: Option<String>,
    pub last_scan_unix_ms: Option<u64>,
    pub last_scan_error: Option<String>,
}
pub struct MirrorLedger { path: PathBuf, pub data: MirrorLedgerV1 }
impl MirrorLedger {
    pub fn path_for(library_root: &Path) -> PathBuf;
    pub fn open(library_root: &Path) -> Result<Self, String>;   // missing = empty; corrupt or v != 1 fails closed
    pub fn put(&mut self, e: MirrorEntryV1) -> Result<(), String>;
    pub fn set_folders(&mut self, amazon: Option<String>, removed: Option<String>) -> Result<(), String>;
    pub fn note_scan(&mut self, error: Option<String>) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorAction {
    Create { source: SourceNotebook, stamp: SourceStamp },
    Rebuild { source: SourceNotebook, stamp: SourceStamp },        // stamp differs from entry
    Rename { source: SourceNotebook },                             // same stamp, title differs
    Restore { source: SourceNotebook, stamp: SourceStamp },        // entry Removed, source is back
    Remove { origin_id: String },                                  // entry Current, source gone
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorScan { pub actions: Vec<MirrorAction>, pub current: usize, pub unsupported: Vec<(String, String)> }
pub fn scan(src: &ImportSources, ledger: &MirrorLedger) -> Result<MirrorScan, String>;
```

- `scan` never decodes. Rules: no entry → `Create`; entry `Current` with different stamp → `Rebuild`; same stamp, different `native_title` → `Rename`; entry `Removed` and source present → `Restore`; entry `Current`/`Unsupported` and source absent → `Remove`; entry `Unsupported` with the same stamp → skipped and listed in `unsupported` (title, error); entry `Rebuilding` → `Rebuild` (recovery handles the partial first, Task 5). A missing catalog or notebooks root yields `Ok(MirrorScan::default())`, not an error, when `discover` returns an empty list; `discover`'s `Err` is returned as-is.

- [ ] **Step 1: Write failing ledger and stamp tests in `mirror_tests.rs`**

```rust
#[test] fn ledger_round_trips_and_a_corrupt_ledger_fails_closed() {
    let s = Scribe::new();
    let mut l = MirrorLedger::open(s.lib.root()).unwrap();
    l.put(entry("a", MirrorState::Current)).unwrap();
    assert_eq!(MirrorLedger::open(s.lib.root()).unwrap().data, l.data);
    fs::write(MirrorLedger::path_for(s.lib.root()), b"{").unwrap();
    assert!(MirrorLedger::open(s.lib.root()).unwrap_err().contains("REFUSE MIRROR"));
}
#[test] fn stamp_changes_when_actions_log_or_nbk_changes_and_ignores_title() {
    let k = Kindle::new(); k.add(ID_A, scratch(), "One");
    let a = stamp_source(&k.root.join(ID_A)).unwrap();
    k.touch(ID_A);
    assert_ne!(a, stamp_source(&k.root.join(ID_A)).unwrap());
}
```

Non-sample tests use a synthetic source: `Kindle::add_synthetic(id, title)` (add to the kit) writes `nbk` = `b"SQLite format 3\0 not a notebook"` so stamp/scan tests need no private samples; `stamp_changes...` uses it too.

- [ ] **Step 2: Write failing scan tests**

```rust
#[test] fn scan_classifies_create_rebuild_rename_restore_and_remove() { /* one Kindle kit, five ids, one ledger entry per rule; assert the exact Vec<MirrorAction> sorted by origin_id */ }
#[test] fn scan_with_missing_catalog_and_root_is_empty_not_an_error() {
    let k = Kindle::new(); fs::remove_dir_all(&k.root).unwrap(); fs::remove_file(&k.catalog).unwrap();
    let s = Scribe::new(); let l = MirrorLedger::open(s.lib.root()).unwrap();
    assert_eq!(scan(&k.sources(), &l).unwrap(), MirrorScan::default());
}
#[test] fn scan_skips_unsupported_sources_until_their_stamp_changes() { /* entry Unsupported + same stamp → listed, no action; touch → Rebuild */ }
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p scribe-app --lib --locked mirror`
Expected: FAIL to compile (`mirror` module missing).

- [ ] **Step 4: Implement `mirror.rs` ledger, stamp and scan per the interfaces above**

Error prefix for a refused ledger is `REFUSE MIRROR:`. `put` keeps memory equal to disk on write failure, like `nbk_import::Ledger::put`.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p scribe-app --lib --locked mirror`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
git add ignite/kindle/scribe-app/src/mirror.rs ignite/kindle/scribe-app/src/mirror_tests.rs ignite/kindle/scribe-app/src/nbk_test_kit.rs ignite/kindle/scribe-app/src/lib.rs
git commit -m "feat(mirror): ledger, source stamp and stat-only scan for Amazon notebooks"
```

---

### Task 3: Content hash and decode classification

**Files:**
- Modify: `ignite/kindle/scribe-app/src/mirror.rs`, `mirror_tests.rs`

**Interfaces:**
- Produces: `pub fn content_hash(nb: &DecodedNotebook) -> String` (lowercase hex SHA-256 over, per page in order: `page_id`, `width`, `height`, then per stroke `xs`, `ys`, `pressure` (absent = empty), `brush_type`, `color`, `thickness` bits; `diagnostics`, `template_ids`, `book_id` excluded); `pub enum DecodeFailure { Transient(String), Permanent(String) }`; `pub fn classify_decode_error(e: &NbkError) -> DecodeFailure` (`SourceBusy`, `SourceChanged`, `PendingActions`, `Io` → Transient; `Container`, `Unsupported` → Permanent).

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn content_hash_is_stable_and_ignores_diagnostics_and_book_id() { /* two DecodedNotebook values built by hand differing only in diagnostics/book_id hash equal; moving one point changes it */ }
#[test] fn decode_errors_split_into_transient_and_permanent() {
    assert!(matches!(classify_decode_error(&NbkError::PendingActions { nbk_strokes: 1, action_stroke_inserts: 2 }), DecodeFailure::Transient(_)));
    assert!(matches!(classify_decode_error(&NbkError::Unsupported("x".into())), DecodeFailure::Permanent(_)));
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p scribe-app --lib --locked mirror::` → FAIL (functions missing).
- [ ] **Step 3: Implement** using the `sha2` crate already in the workspace.
- [ ] **Step 4: Run to verify they pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat(mirror): deterministic content hash and decode error classes"`

---

### Task 4: Folder helpers and sync hooks

**Files:**
- Modify: `ignite/kindle/scribe-app/src/mirror.rs`, `mirror_tests.rs`
- Modify: `ignite/kindle/scribe-app/src/runtime.rs` (`enum FolderCapture` and `fn capture_folder_change` become `pub(crate)`)

**Interfaces:**
- Produces:

```rust
/// Mirror mutations beyond what ImportSync covers. All are post-commit captures or prepare/commit pairs
/// as the runtime does them; NoSync implements every method as a no-op or plain durable write.
pub trait MirrorSync: ImportSync {
    fn delete_page(&self, nb_dir: &Path, manifest: &mut PersistedNotebookManifestV1, notebook_id: &str, page_id: &str) -> Result<(), String>;
    fn rename_notebook(&self, library: &mut LibraryRepository, notebook_id: &str, title: &str) -> Result<(), String>;
    fn folder_changed(&self, folders: &FolderStore, change: FolderCapture);
}
impl MirrorSync for NoSync { .. }
impl MirrorSync for GlobalSync { .. }   // prepare_delete_page / prepare_rename_notebook + resolve; capture_folder_change
pub fn ensure_folder(ledger: &mut MirrorLedger, folders: &mut FolderStore, live: &[String], which: Which, sync: &dyn MirrorSync) -> Result<String, String>;
pub enum Which { Amazon, Removed }
```

`ensure_folder` returns the ledger-recorded local folder id when `folders.title(id)` is still present, otherwise creates a folder titled `AMAZON_FOLDER_TITLE` / `REMOVED_FOLDER_TITLE`, captures `FolderCapture::Created`, records it in the ledger. A user who renamed the folder keeps their title (the id is what matters).

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn ensure_folder_creates_once_and_reuses_even_after_rename() { /* two calls → same id; folders.rename(id, "Kindle") then third call → same id, title "Kindle" */ }
#[test] fn ensure_folder_recreates_when_the_user_deleted_it() { /* folders.delete(id) → next call returns a new id, ledger updated */ }
```

- [ ] **Step 2: Run to verify they fail.** — `cargo test -p scribe-app --lib --locked mirror::ensure_folder` → FAIL.
- [ ] **Step 3: Implement the trait, both impls, and `ensure_folder`.** `GlobalSync::delete_page` does `with_global(|s| s.prepare_delete_page(..))` → `delete_page_durable(nb_dir, manifest, page_id, &DeletePageHooks::default())` → `commit`/`abort` the ticket, matching `runtime.rs:1419-1432`.
- [ ] **Step 4: Run to verify they pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat(mirror): Amazon/Removed folder helpers and mirror sync hooks"`

---

### Task 5: Rebuild engine and startup recovery

**Files:**
- Modify: `ignite/kindle/scribe-app/src/mirror.rs`, `mirror_tests.rs`

**Interfaces:**
- Consumes: Task 1 `build`, `convert`; Task 2 ledger/scan; Task 3 hash; Task 4 folders and `MirrorSync`.
- Produces:

```rust
pub struct MirrorTarget<'a> { pub import: ImportTarget<'a>, pub folders: &'a mut FolderStore }
pub enum MirrorOutcome { Done, Skipped(String), Failed(String), Halted(String) }
/// Perform one action. `decode` is injected so tests can hand in a DecodedNotebook without private samples.
pub fn apply_one(t: &mut MirrorTarget<'_>, ledger: &mut MirrorLedger, action: &MirrorAction,
                 decode: &dyn Fn(&Path) -> Result<DecodedNotebook, NbkError>, sync: &dyn MirrorSync) -> MirrorOutcome;
pub fn kindle_decode(dir: &Path) -> Result<DecodedNotebook, NbkError>;   // read_source + decode_snapshot
/// Startup: every `Rebuilding` entry's notebook is removed through the synced delete path and the entry
/// reset to stamp None (so the next scan recreates it). Runs before any UI, next to nbk_import recovery.
pub fn recover_interrupted(library: &mut LibraryRepository, sync: &dyn MirrorSync) -> Result<usize, String>;
```

Per action:
- `Create`/`Restore`/`Rebuild`: `sync.flush`; decode; on `Transient` → `Skipped` (ledger untouched); on `Permanent` → entry `Unsupported` with stamp+error (existing mirror notebook kept as-is) → `Failed`. Else `convert`, hash; if hash equals the entry's and action is `Rebuild` → update stamp only, `Done`. Otherwise entry `Rebuilding` (durable) then: `Create`/`Restore` → `build` with marker `(MIRROR_MARKER, json of {v:1, origin_kind, origin_id, mirrored_unix_ms})`; `Rebuild` → insert new pages after the last existing page (`insert_page_after_durable` + `page_created`), append strokes in `STROKES_PER_FSYNC` chunks with `flush` between, then `delete_page` for each old page id in order. Then `set_folder` to the Amazon folder (`Restore` and `Create`), `rename_notebook` if the title differs, entry `Current` with stamp, hash, pages → `Done`.
- `Rename`: `rename_notebook`, update entry title → `Done`.
- `Remove`: `set_folder` to the Removed folder, entry `Removed` → `Done`.
- `Halted` wraps any `ImportSync::flush` error, like `nbk_import`; the caller stops the run.

- [ ] **Step 1: Write failing tests (synthetic decode, no samples)**

A helper `fn decoded(pages: &[&[(i64, i64)]]) -> DecodedNotebook` builds canvas 15624x20832 pages with one stroke each. A `fn decode_from(map: HashMap<PathBuf, Result<DecodedNotebook, NbkError>>) -> impl Fn(&Path) -> ...`.

```rust
#[test] fn create_builds_a_read_only_notebook_in_the_amazon_folder_with_marker_and_ledger_entry()
#[test] fn rebuild_with_the_same_content_hash_only_updates_the_stamp()
#[test] fn rebuild_creates_new_pages_before_deleting_old_ones_and_never_drops_below_one_page()
    // use the kit's recording ImportSync (from nbk_import_tests) and assert the op sequence
#[test] fn rebuild_restores_title_and_folder()
#[test] fn transient_decode_errors_leave_the_ledger_untouched()
#[test] fn permanent_decode_errors_mark_unsupported_and_keep_the_old_mirror()
#[test] fn remove_files_the_mirror_under_removed_and_restore_brings_it_back()
#[test] fn recovery_after_a_crash_mid_rebuild_removes_the_partial_and_recreates_once()
    // put entry Rebuilding + partial notebook; recover_interrupted → notebook gone; scan → Create; apply → exactly one mirror notebook
#[test] fn the_source_tree_is_never_written()
    // snapshot every path + len + mtime under the Kindle kit root and catalog before scan/apply (all action kinds, including a Permanent failure); identical after
```

- [ ] **Step 2: Run to verify they fail.** — `cargo test -p scribe-app --lib --locked mirror::` → FAIL.
- [ ] **Step 3: Implement `apply_one`, `kindle_decode`, `recover_interrupted`.**
- [ ] **Step 4: Run the whole scribe-app suite.** — `cargo test -p scribe-app --lib --locked` → PASS.
- [ ] **Step 5: Add one ignored sample-backed test** `mirrors_a_real_notebook_with_exact_geometry` mirroring `imports_title_pages_and_exact_geometry_into_a_native_notebook`, using `kindle_decode`. Run with `-- --ignored` only when `IGNITE_NBK_IMPORT_SAMPLES` is set; it is documentation of the hardware path, not a CI gate.
- [ ] **Step 6: Commit** — `git commit -m "feat(mirror): rebuild engine with crash recovery"`

---

### Task 6: Runtime integration (triggers, safe point, startup, pen gate)

**Files:**
- Modify: `ignite/kindle/scribe-app/src/runtime.rs`
- Modify: `ignite/kindle/scribe-app/src/prefs.rs`
- Test: `ignite/kindle/scribe-app/src/mirror_tests.rs` (pure scheduler tests) and `prefs.rs` unit tests

**Interfaces:**
- Produces in `mirror.rs` (pure, testable):

```rust
pub struct MirrorRun { pub queue: VecDeque<MirrorAction>, pub done: usize, pub failed: Vec<(String, String)> }
pub struct MirrorClock { pub last_scan: Option<Instant>, pub last_batches: u64 }
pub const IDLE_RESCAN: Duration = Duration::from_secs(15 * 60);
/// Should a scan start now? launch (last_scan None), upload batch count advanced, or idle timer elapsed.
pub fn scan_due(clock: &MirrorClock, now: Instant, batches: u64) -> bool;
/// May a rebuild step run now? Home or Tools only, no import run, sync linked and capture-ready.
pub fn step_allowed(view: AppView, import_running: bool, sync_linked: bool) -> bool;
```

- Produces in `prefs.rs`: `mirror_amazon: bool` field on `PreferencesFile` with `#[serde(default = "default_true")]`; `pub fn load_mirror_enabled(path) -> bool`; `pub fn save_mirror_enabled(path, bool) -> Result<(), String>` (same atomic write; keep `side_button`).

Runtime wiring, all at the structural safe point block in `runtime.rs` near line 4195, in this order after the existing `import_step` block:
1. `if structure_eligible && pending_ui.is_none() && !ui.modal_open() && mirror_run.is_none() && prefs_mirror && scan_due(&clock, now, with_global(|s| s.stats().batches).unwrap_or(0))` → run `mirror::scan`; on `Ok` queue a `MirrorRun`, `ledger.note_scan(None)`; on `Err(e)` `note_scan(Some(e))` and log `mirror_scan_failed detail=`. Update `clock`.
2. `if ... && mirror_run.is_some() && step_allowed(ui.view, import_run.is_some(), with_global(|_| ()).is_some())` → `mirror_step` applies one action with `GlobalSync`, pops it, logs `mirror_step action=.. outcome=..`; on `Halted` drops the run. A step while `ui.view == AppView::Notebook` is impossible by the gate.
3. Startup: call `mirror::recover_interrupted` right after the existing `nbk_import` startup recovery.
4. Pen gate: where `AppView::allows_page_input` admits a stroke, also check `ctrl.session_read_only` (new `bool` set when a notebook is opened if `notebook_dir.join(MIRROR_MARKER).is_file()`); when set, discard the stroke and show the one-line modal `"This notebook mirrors Amazon and is read-only. Use Copy to Ignite from Home."` once per open.

- [ ] **Step 1: Write failing pure tests**

```rust
#[test] fn scan_is_due_at_launch_after_an_upload_batch_and_after_fifteen_idle_minutes()
#[test] fn a_rebuild_never_runs_while_a_page_is_open() { for v in [AppView::Notebook, AppView::Sync, AppView::Preferences, AppView::LinkTargetPicker] { assert!(!step_allowed(v, false, true)); } assert!(step_allowed(AppView::Home, false, true)); assert!(!step_allowed(AppView::Home, true, true)); assert!(!step_allowed(AppView::Tools, false, false)); }
#[test] fn a_scan_error_never_removes_existing_mirrors() { /* ledger with one Current entry; scan over a Kindle whose catalog file is unreadable (a directory) → Err; ledger entries unchanged */ }
#[test] fn preferences_default_mirror_on_and_round_trip_with_side_button()  // prefs.rs
```

- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement the pure functions and prefs field; wire the runtime.**
- [ ] **Step 4: Run** `cargo test -p scribe-app --lib --locked` → PASS; `cargo build -p scribe-app --locked` → no warnings in `mirror.rs`.
- [ ] **Step 5: Commit** — `git commit -m "feat(mirror): schedule scans and rebuilds at the structural safe point; read-only pen gate"`

---

### Task 7: Tools and Preferences UI, Home "Copy to Ignite"

**Files:**
- Modify: `ignite/kindle/scribe-app/src/tools_ui.rs`, `runtime.rs`, `settings_ui.rs` (Preferences rows), `app_ui.rs` (`UiCommand` variants `MirrorRescan`, `SetMirrorEnabled(bool)`, `CopyMirror(String)` and the Home card menu), `modal.rs` (the read-only hint text)

**Interfaces:**
- Produces: `pub struct MirrorSummary { pub enabled: bool, pub current: usize, pub pending: usize, pub last_scan_unix_ms: Option<u64>, pub last_scan_error: Option<String>, pub unsupported: Vec<(String, String)> }` on `ToolsState` (`pub mirror: MirrorSummary`), built from the ledger by `mirror::summary(ledger, enabled) -> MirrorSummary`; `status_lines` appends a `Mirror` block: `"Amazon mirror: on, 12 current, 1 pending, last scan 3 min ago"` and up to five `"<title>: <failure_reason(error)>"` lines; a Tools button `Rescan now` → `UiCommand::MirrorRescan` (forces `scan_due` by clearing `clock.last_scan`).
- Preferences: row `Mirror Amazon notebooks: On/Off` toggling `save_mirror_enabled`.
- Home: long-press menu on a card whose dir has `MIRROR_MARKER` offers `Copy to Ignite` → `UiCommand::CopyMirror(notebook_id)` → reads the marker's `origin_id`, rebuilds a `SourceNotebook` from the catalog, calls `nbk_import::import_one` with `GlobalSync`; `AlreadyImported` shows `"Already copied to Ignite."`.

- [ ] **Step 1: Write failing tests** for `status_lines` with a populated `MirrorSummary` (exact strings above) and for `mirror::summary`.
- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** following the existing `ToolsPhase` rendering and `apply_side_button` patterns.
- [ ] **Step 4: Run** `cargo test -p scribe-app --lib --locked` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(mirror): Tools status, Preferences toggle and Copy to Ignite"`

---

### Task 8: Browser companion treats the `Amazon` folder as read-only

**Files:**
- Create: `ignite/web/src/model/mirror.ts`, `ignite/web/tests/mirror.test.ts`
- Modify: `ignite/web/src/components/Viewer.tsx`

**Interfaces:**
- Produces: `export const AMAZON_FOLDER_TITLE = "Amazon";` `export function isAmazonMirror(snapshot: Pick<SyncSnapshotV1, "folders">, notebookId: string): boolean` (true when a folder titled exactly `Amazon` lists the notebook).
- Viewer: when `isAmazonMirror(viewSnapshot, currentNotebookId)`, the ink and type mode buttons are not rendered, `uiMode` is forced to `"read"` on notebook switch, and a banner `Mirrored from Amazon. Read-only here; write in Ignite or copy it on the Kindle.` shows above the page.

- [ ] **Step 1: Write failing vitest**

```ts
describe("isAmazonMirror", () => {
  it("is true only for notebooks listed by a folder titled Amazon", () => {
    const snap = { folders: [{ folder_id: F1, title: "Amazon", notebook_ids: [NB1] }, { folder_id: F2, title: "Work", notebook_ids: [NB2] }] };
    expect(isAmazonMirror(snap, NB1)).toBe(true);
    expect(isAmazonMirror(snap, NB2)).toBe(false);
    expect(isAmazonMirror({ folders: [] }, NB1)).toBe(false);
  });
});
```

- [ ] **Step 2: Run** `npm test -- mirror` in `ignite/web` → FAIL (module missing).
- [ ] **Step 3: Implement the model and the Viewer gating.**
- [ ] **Step 4: Run** `npm test && npm run typecheck && npm run lint` → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(web): Amazon folder is read-only in the companion"`

---

### Task 9: Docs

**Files:**
- Modify: `docs/architecture.md` (new "Amazon mirror" paragraph under "Durable notebook storage": ledger path, folder convention, safe-point rule), `docs/troubleshooting.md` ("Amazon notebook not appearing": pending actions, unsupported list in Tools, mirror toggle, sync must be linked).

- [ ] **Step 1: Write both sections.**
- [ ] **Step 2: Commit** — `git commit -m "docs: Amazon mirror architecture and troubleshooting"`

---

### Task 10: Hardware verification (manual gate, not CI)

On the verified Scribe (FW 5.18.6) with sync linked:

- [ ] Write a page in Amazon's notebook app, return to Ignite Home: within one scan the notebook appears under `Amazon` in the browser companion with matching strokes.
- [ ] `ls -la --time-style=full-iso /mnt/us/.notebooks/<uuid>/` before and after: no new files, no mtime change, no `nbk-journal`.
- [ ] Open the mirror on the Kindle and try to write: the read-only hint shows, no stroke lands.
- [ ] Delete the notebook in Amazon's app: it moves to `Removed from Amazon`.
- [ ] Record results in the release qualification notes.
