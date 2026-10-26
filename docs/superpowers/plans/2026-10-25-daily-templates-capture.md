# Templates, Daily Page, Quick Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Page templates (built-in and custom PNG), a monthly daily-page journal with typed date stamps, and a side-button quick-capture inbox, all on the Kindle, with no sync-protocol change.

**Architecture:** A new pure crate `page-templates` renders parametric backgrounds; a Kindle-side `templates` module caches rendered or decoded bitmaps by hash under the library root and feeds the existing notebook-create and page-insert paths. A `text_author` module lets the Kindle emit `AddText` (the protocol already has it) for date and time stamps. `daily` and `capture` modules keep small replica-local ledgers that resolve reserved notebooks by id and recreate them when gone. Home gains `Today` and `Capture` tiles; Preferences gains a third side-button action and an open-at-launch choice.

**Tech Stack:** Rust (scribe-app, new `page-templates` crate, `png` 0.17 pure-Rust decoder, `libc` for `localtime_r`), existing test kits.

**Spec:** `docs/superpowers/specs/2026-10-25-daily-templates-capture-design.md`

## Global Constraints

- No change to `SyncOperationV1`, `SyncText`, `BackgroundRef`, `ReceiptsV1`, `FoldersFileV1`, or `preferences.json` versions (`v` stays 1; new fields use serde defaults).
- Nothing new runs on the pen path; template rendering, PNG decoding, ledger I/O and text authoring happen at command time (Home, modals, page menu) or at the structural safe point.
- Panel is 1860 x 2480 gray8. Every built-in template and every letterboxed custom template keeps rows `0..130` (chrome bar) and rows `2480-57..2480` plain white (`0xFF`), so `scrub_chrome_band` on page copy leaves the bitmap unchanged and every page of a notebook shares one blob hash.
- Line ink value for built-ins is `0xC0`; dots `0xA0`; everything else white.
- Template files live under the library root (`--data-dir`, default `/mnt/us/scribe-data`): `templates/cache/<sha256>.gray8`, `templates/cache/<sha256>.thumb.gray8`, `templates/custom/*.png`.
- Ledgers: `<data-dir>/.scribe-daily/daily.json`, `<data-dir>/.scribe-capture/capture.json`; atomic writes via `nbk_import::write_atomic`; corrupt ledgers fail closed with prefix `REFUSE DAILY:` / `REFUSE CAPTURE:` and the feature shows an error modal instead of acting.
- Reserved notebook titles: monthly `Daily 2026-10` (year-month), folder `Daily`, inbox `Inbox`. Date stamp `Saturday 25 October 2026`; capture stamp `Sat 25 Oct 14:32`. Clock guard text `Set the Kindle clock first.`; date before 2025-01-01 counts as unset.
- Local time via libc `localtime_r`; `scribe-app` gains `libc = "0.2"`.
- Pure-Rust dependencies only; `png = "0.17"` added to `scribe-app` (its `miniz_oxide` backend is already in the lock).
- Conventional commits ending with the session's attribution line; `cargo test -p scribe-app --lib --locked`, `cargo test -p page-templates --locked` and `cargo build -p scribe-app --locked` (no new warnings in touched files) pass at every commit. Known host-only failure `real_procfs_loopback_listener_then_refuse_after_exit` and two timing flakes are not this plan's.

## Review Focus

1. **A custom PNG that is RGBA, 16-bit, indexed, or interlaced.** Expected: it still decodes to a letterboxed gray8 template or is listed as unreadable; never a panic or a half-written cache file. Test in Task 2 (`custom_png_variants_decode_or_are_reported`).
2. **Midnight passes while today's daily page is open.** Expected: nothing happens on its own; the next `Today` creates tomorrow's page after the current one. Test in Task 6 (`today_after_rollover_appends_a_new_page_once`).
3. **The owner deletes or renames `Inbox` or a `Daily …` notebook.** Expected: the next capture or `Today` recreates what is missing and keeps using what was merely renamed. Test in Tasks 6 and 7 (`ledger_re_resolves_renamed_and_recreates_deleted`).
4. **Side-button capture pressed on Home, in a modal, or already on the Inbox's fresh page.** Expected: on Home and in modals the side button has no page to land on, so nothing happens; on a fresh Inbox page it returns to the origin (or stays, if there is none). Test in Task 7 (`capture_dispatch_is_a_no_op_without_a_page_and_returns_from_inbox`).
5. **A template whose render or decode is slow on the ARM device.** Expected: one render per distinct template ever, never on the pen path, with the cache reused across launches. Pinned by Task 2's cache test (`bitmap_renders_once_and_reuses_the_cache`) and measured in the Task 10 hardware gate.

---

## File structure

- Create `ignite/core/page-templates/` (crate): `src/lib.rs` (`TemplateSpec`, `render`, `thumbnail`, safe-area constants, `BUILTINS`), `tests/golden.rs`.
- Create `ignite/kindle/scribe-app/src/templates.rs` + `templates_tests.rs`: Kindle cache, custom PNG decoding, listing, last-used preference.
- Create `ignite/kindle/scribe-app/src/text_author.rs` + tests: Kindle-authored `SyncText` stamps with the outbound ticket.
- Create `ignite/kindle/scribe-app/src/clock.rs`: local date/time and formatting.
- Create `ignite/kindle/scribe-app/src/daily.rs` + `daily_tests.rs`, `capture.rs` + `capture_tests.rs`: ledgers and the two flows.
- Modify `modal.rs` (template picker, page-menu button), `app_ui.rs` (`UiCommand` variants, Home tile hits, picker resolution), `home.rs` (`Today`/`Capture` tiles), `settings_ui.rs` (third side-button row, open-at-launch section), `prefs.rs`, `nav.rs` (`Capture` dispatch), `notebook.rs` (`insert_page_after_with_background`), `sync_outbound.rs` (`prepare_add_text`), `runtime.rs` (command arms, launch hook, return slot), `Cargo.toml` files, `lib.rs`.
- Docs: `README.md` features, `docs/architecture.md`, `docs/troubleshooting.md`.

---

### Task 1: `page-templates` crate

**Files:**
- Create: `ignite/core/page-templates/Cargo.toml`, `src/lib.rs`, `tests/golden.rs`
- Modify: `Cargo.toml` (workspace `members` and `default-members`)

**Interfaces:**
- Produces:

```rust
pub const PANEL_W: u32 = 1860; pub const PANEL_H: u32 = 2480;
pub const SAFE_TOP: u32 = 130; pub const SAFE_BOTTOM: u32 = 57;
pub const LINE: u8 = 0xC0; pub const DOT: u8 = 0xA0; pub const WHITE: u8 = 0xFF;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplateSpec {
    Blank, Lined { spacing_px: u32, margin_px: u32 }, DotGrid { pitch_px: u32 }, Graph { pitch_px: u32 },
    Cornell, Daily, Weekly, Isometric { pitch_px: u32 }, MusicStaff,
}
pub struct Builtin { pub id: &'static str, pub name: &'static str, pub spec: TemplateSpec }
pub const BUILTINS: &[Builtin];   // ids: blank, lined, dot_grid, graph, cornell, daily, weekly, isometric, music_staff (in this order)
pub fn render(spec: &TemplateSpec, w: u32, h: u32) -> Vec<u8>;             // gray8, len w*h
pub fn thumbnail(bitmap: &[u8], w: u32, h: u32, factor: u32) -> Vec<u8>;   // box-average downscale
pub fn sha256_hex(bytes: &[u8]) -> String;
```

- Values: `Lined { spacing_px: 94, margin_px: 160 }`, `DotGrid { pitch_px: 60 }`, `Graph { pitch_px: 60 }`, `Isometric { pitch_px: 80 }` for the built-ins; `Daily` = a 2 px `LINE` rule at row `SAFE_TOP + 300` with `Lined` below it; `Cornell` = cue column divider at x = 560 and summary rule at row `PANEL_H - SAFE_BOTTOM - 420`; `Weekly` = seven equal bands between the safe rows separated by 2 px rules; `MusicStaff` = groups of five 1 px lines 24 px apart, groups every 260 px. Authors' defaults; the owner may retune later.

- [ ] **Step 1: Write failing golden tests** in `tests/golden.rs`

```rust
#[test] fn every_builtin_renders_the_panel_size_and_keeps_the_safe_rows_white() {
    for b in BUILTINS { let px = render(&b.spec, PANEL_W, PANEL_H);
        assert_eq!(px.len(), (PANEL_W * PANEL_H) as usize);
        assert!(px[..(PANEL_W * SAFE_TOP) as usize].iter().all(|&v| v == WHITE), "{}", b.id);
        assert!(px[((PANEL_H - SAFE_BOTTOM) * PANEL_W) as usize..].iter().all(|&v| v == WHITE), "{}", b.id); } }
#[test] fn renders_are_deterministic_and_distinct() { /* sha256 of each builtin equal across two renders; all nine hashes distinct; blank is all WHITE */ }
#[test] fn lined_puts_rules_at_the_spacing_and_nowhere_else() { /* row SAFE_TOP+spacing is LINE from margin to w-margin; row SAFE_TOP+spacing/2 is WHITE */ }
#[test] fn thumbnail_box_averages_by_factor() { /* 16x16 checker, factor 8 → 2x2 of 0x80-ish values (±1) */ }
```

- [ ] **Step 2: Run** `cargo test -p page-templates --locked` → FAIL (crate missing).
- [ ] **Step 3: Implement the crate** (dependencies: `serde` with derive, `sha2`); add it to workspace `members` and `default-members`.
- [ ] **Step 4: Run** `cargo test -p page-templates --locked` → PASS.
- [ ] **Step 5: Commit** — `feat(page-templates): parametric page backgrounds with safe chrome rows`

---

### Task 2: Kindle template store (cache, custom PNG, listing, last-used)

**Files:**
- Create: `ignite/kindle/scribe-app/src/templates.rs`, `templates_tests.rs`
- Modify: `scribe-app/Cargo.toml` (`page-templates`, `png = "0.17"`), `lib.rs`, `prefs.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces:

```rust
pub const TEMPLATES_DIR: &str = "templates"; pub const CACHE_DIR: &str = "cache"; pub const CUSTOM_DIR: &str = "custom";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplateId { Builtin { id: String }, Custom { name: String } }   // name = file stem
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateStatus { Ready, Unreadable(String) }
pub struct TemplateEntry { pub id: TemplateId, pub name: String, pub status: TemplateStatus }
pub fn list(data_dir: &Path) -> Vec<TemplateEntry>;            // builtins in BUILTINS order, then custom by file name; never fails
pub fn bitmap(data_dir: &Path, id: &TemplateId, geo: &FbGeometry) -> Result<Vec<u8>, String>;  // cached by sha256 of the rendered/decoded bytes; writes cache atomically
pub fn thumbnail(data_dir: &Path, id: &TemplateId, geo: &FbGeometry) -> Result<Vec<u8>, String>; // factor 8, cached beside
pub fn decode_custom_png(bytes: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String>;  // grayscale (luma), letterboxed centred into the safe area, white elsewhere
// prefs.rs
pub fn load_template(path: &Path) -> TemplateId;              // default Builtin{id:"blank"}; unknown/malformed → default
pub fn save_template(path: &Path, id: &TemplateId) -> Result<(), String>;   // preserves the other fields
```

- Cache key: the sha256 of the final gray8 bytes (so a builtin spec change or a changed PNG yields a new file; stale files are left alone). A cache index `templates/cache/index.json` maps `TemplateId` → sha256 so `bitmap` can check for a cached file without rendering; a missing or corrupt index is simply rebuilt.

- [ ] **Step 1: Write failing tests** (`templates_tests.rs`, `#[path]` module like `mirror_tests.rs`; temp data dirs via `nbk_test_kit::tmp`)

```rust
#[test] fn list_has_all_builtins_then_custom_files_sorted_with_unreadable_marked()
#[test] fn bitmap_renders_once_and_reuses_the_cache()   // count files in cache after two calls == 1 (+ index); mtime unchanged on second call
#[test] fn custom_png_variants_decode_or_are_reported()  // write RGBA, 16-bit gray, indexed, interlaced PNGs with the `png` encoder; all decode to PANEL_W*PANEL_H gray8 with SAFE rows white; a truncated file → Unreadable in list and Err from bitmap, no cache file written
#[test] fn custom_png_is_letterboxed_into_the_safe_area() // 100x300 black PNG → centred column of 0x00 between SAFE_TOP and PANEL_H-SAFE_BOTTOM, white outside
#[test] fn template_preference_round_trips_and_keeps_side_button_and_mirror()
```

- [ ] **Step 2: Run** `cargo test -p scribe-app --lib --locked templates` → FAIL.
- [ ] **Step 3: Implement** `templates.rs` and the prefs field `template: TemplateId` with `#[serde(default)]`.
- [ ] **Step 4: Run** focused then `cargo build -p scribe-app --locked` (no new warnings) → PASS.
- [ ] **Step 5: Commit** — `feat(templates): Kindle template cache with custom PNG support`

---

### Task 3: Template picker and create/insert flows

**Files:**
- Modify: `modal.rs`, `app_ui.rs`, `notebook.rs`, `runtime.rs`, `modal`/`app_ui` tests (`f11_tests.rs`, `folder_ui_tests.rs` where button lists are asserted)

**Interfaces:**
- Consumes: Task 2 (`list`, `bitmap`, `load_template`, `save_template`).
- Produces:

```rust
// modal.rs
pub enum PickerPurpose { NewNotebook { title: String, folder_id: Option<String> }, InsertPage }
Modal::TemplatePicker { purpose: PickerPurpose, options: Vec<(TemplateId, String)>, selected: usize, page: usize }
ButtonId::PickTemplate(usize); ButtonId::InsertPageWithTemplate   // page menu: "INSERT PAGE WITH TEMPLATE" after "INSERT PAGE AFTER"
// app_ui.rs
UiCommand::CreateNotebookWithTemplate { title: String, folder_id: Option<String>, template: TemplateId }
UiCommand::InsertPageWithTemplate(TemplateId)
// notebook.rs
pub fn insert_page_after_with_background(data_dir: &Path, manifest: &mut PersistedNotebookManifestV1, after_page_id: &str, background: &[u8], hooks: &InsertPageHooks) -> Result<InsertPageCommit, String>;
```

- Flow: `TitleEntry` submit for `TitlePurpose::Create` / `CreateNotebookInFolder` no longer emits `CreateNotebook`; it opens `TemplatePicker { purpose: NewNotebook {..}, selected: index of load_template, .. }` (paged like `FolderPicker`, `PICKER_PAGE` per page, the selected option's label suffixed ` ✓`, unreadable customs disabled). `PickTemplate(i)` → `CreateNotebookWithTemplate` (and `save_template`). Page menu `InsertPageWithTemplate` button → picker with `InsertPage` → `InsertPageWithTemplate(id)`. Runtime arms call `create_notebook_durable(.., &bitmap, ..)` / `insert_page_after_with_background` + the existing `page_created` sync capture (the background blob is derived from the page files, so nothing else changes). The former `CreateNotebook(String)` arms keep working for callers that still use them (`NewChooser` for folders, tests).
- Thumbnails are not shown in v1 (the dialog system is text buttons); the spec's thumbnail grid is deferred, and `thumbnail()` from Task 2 stays for that later pass. Plan author's call.

- [ ] **Step 1: Write failing tests** in `app_ui.rs` / `modal.rs` test modules: `title_submit_opens_the_template_picker_on_the_last_used_template`, `pick_template_creates_with_that_template_and_remembers_it`, `page_menu_offers_insert_page_with_template`, and in `notebook.rs`: `insert_page_after_with_background_uses_the_given_bitmap` (page files' `background.raw` equals the input; manifest order correct).
- [ ] **Step 2: Run** `cargo test -p scribe-app --lib --locked app_ui modal notebook` → FAIL.
- [ ] **Step 3: Implement** the modal, commands, notebook helper and runtime arms.
- [ ] **Step 4: Run** focused tests, then the full `cargo test -p scribe-app --lib --locked` → PASS.
- [ ] **Step 5: Commit** — `feat(templates): pick a template when creating a notebook or inserting a page`

---

### Task 4: Kindle-authored text stamps

**Files:**
- Create: `ignite/kindle/scribe-app/src/text_author.rs` (+ tests in the same file's `mod tests`)
- Modify: `sync_outbound.rs`, `lib.rs`

**Interfaces:**
- Consumes: `store::{load_page_texts, write_page_texts}`, `text_paint::load_texts_and_overlay`, the `prepare_add_page_link` ticket pattern.
- Produces:

```rust
// sync_outbound.rs
pub fn prepare_add_text(&self, notebook_local: &str, page_local: &str, text: &SyncText) -> Option<PrepTicket>;  // AddText { notebook_id, page_id, text }
// text_author.rs
pub const STAMP_X: f32 = 1_344.0;      // 160 px left margin in canonical units
pub const STAMP_Y: f32 = 1_512.0;      // below the chrome bar (130 px) plus 50 px, canonical
pub const STAMP_FONT_SIZE: f32 = /* the browser's DEFAULT_TEXT_SESSION_FONT_SIZE value, copied verbatim with a comment naming its source */;
pub fn stamp_page(nb_dir: &Path, notebook_local: &str, page_local: &str, content: &str) -> Result<SyncText, String>;
```

- `stamp_page`: mint `TextId::from_uuid(Uuid::now_v7())`, width = `CANONICAL_WIDTH - 2 * STAMP_X`, face `DefaultSansV1`, validate; prepare the ticket (sync off → none; `Some(None)` → `Err` without writing); append to `texts.jsonl` via `write_page_texts`; commit or abort the ticket exactly as the link path does. The caller repaints (overlay rebuild is the existing page-open path).

- [ ] **Step 1: Write failing tests**: `stamp_page_appends_a_valid_text_and_round_trips` (texts.jsonl has one line, `validate()` ok, content exact), `stamp_page_with_sync_off_writes_locally`, and, mirroring the existing link capture test if one runs without a live server, `prepare_add_text_captures_an_add_text_envelope` (else inspection-only, stated in the report).
- [ ] **Step 2: Run** `cargo test -p scribe-app --lib --locked text_author` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** focused → PASS; `cargo build` no new warnings.
- [ ] **Step 5: Commit** — `feat(text): Kindle-authored text stamps via AddText`

---

### Task 5: Local clock

**Files:**
- Create: `ignite/kindle/scribe-app/src/clock.rs`
- Modify: `scribe-app/Cargo.toml` (`libc = "0.2"`), `lib.rs`

**Interfaces:**
- Produces:

```rust
pub struct LocalDateTime { pub year: i32, pub month: u32, pub day: u32, pub weekday: u32 /* 0 = Sunday */, pub hour: u32, pub minute: u32 }
pub const MIN_TRUSTED_YEAR: i32 = 2025;
pub fn now_local() -> Option<LocalDateTime>;                 // None when year < MIN_TRUSTED_YEAR
pub fn local_from_unix(secs: i64) -> Option<LocalDateTime>;  // libc::localtime_r; test seam
pub fn ymd(d: &LocalDateTime) -> String;        // "2026-10-25"
pub fn ym(d: &LocalDateTime) -> String;         // "2026-10"
pub fn long_date(d: &LocalDateTime) -> String;  // "Saturday 25 October 2026"
pub fn stamp(d: &LocalDateTime) -> String;      // "Sat 25 Oct 14:32"
```

- [ ] **Step 1: Write failing tests** (set `TZ=UTC` and call `libc::tzset` in the test): `formats_match_the_spec_for_a_known_instant` (1_792_950_000 under `TZ=UTC` → 2026-10-25, weekday 0, "Sunday 25 October 2026", "Sun 25 Oct 17:40"), `years_before_2025_are_untrusted` (epoch 0 → None).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat(clock): local date and time via localtime_r`

---

### Task 6: Daily page

**Files:**
- Create: `ignite/kindle/scribe-app/src/daily.rs`, `daily_tests.rs`
- Modify: `home.rs` (`HomeHit::Today`, `HomeLayout.today_button`), `app_ui.rs` (`UiCommand::OpenToday`), `prefs.rs` (`open_at_launch`), `settings_ui.rs` (section `OPEN AT LAUNCH`, rows `Home` / `Today`, `PreferencesHit::LaunchHome | LaunchToday`, `UiCommand::SetOpenAtLaunch(OpenAtLaunch)`), `runtime.rs`, `lib.rs`

**Interfaces:**
- Consumes: Task 2 (`bitmap` of `Builtin{id:"daily"}`), Task 4 (`stamp_page`), Task 5, `mirror::ensure_folder`-style folder handling (`FolderStore`, `capture_folder_change`).
- Produces:

```rust
pub const DAILY_DIR: &str = ".scribe-daily"; pub const DAILY_FILE: &str = "daily.json";
pub const DAILY_FOLDER_TITLE: &str = "Daily";
#[derive(Default, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
pub struct DailyLedgerV1 { pub v: u32, pub folder: Option<String>, pub months: BTreeMap<String, String> /* "2026-10" → notebook id */, pub days: BTreeMap<String, (String, String)> /* "2026-10-25" → (notebook, page) */ }
pub struct DailyLedger { /* open/put like MirrorLedger, prefix REFUSE DAILY: */ }
pub fn month_title(ym: &str) -> String;   // "Daily 2026-10"
pub enum TodayOutcome { Opened { notebook_id: String, page_id: String, created: bool }, ClockUnset }
pub fn open_today(x: &mut UiExec<'_>, ctrl: &mut KindleCtrl) -> Result<TodayOutcome, String>;   // in runtime.rs, calling daily::resolve_today
pub fn resolve_today(ledger: &mut DailyLedger, library: &mut LibraryRepository, folders: &mut FolderStore, now: &LocalDateTime, create: &mut dyn FnMut(CreateWhat) -> Result<String, String>) -> Result<(String, String, bool), String>;
pub enum CreateWhat { Folder, Month { title: String }, Day { notebook_id: String, after_page: String } }
// prefs.rs
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum OpenAtLaunch { #[default] Home, Today }
pub fn load_open_at_launch(path) -> OpenAtLaunch; pub fn save_open_at_launch(path, OpenAtLaunch) -> Result<(), String>;
```

- `resolve_today` is pure over the ledger and the library manifest: re-resolve `folder` (exists by id → keep, else create), `months[ym]` (exists → keep, else create with the `daily` template and file into the folder; its `page-1` becomes `days[ymd]`), `days[ymd]` (exists → open, else insert a page after the notebook's last page and stamp it with `long_date`). Creation itself is injected (`create`) so the test kit can run it without the runtime; the runtime closure uses `create_notebook_durable` + `insert_page_after_durable` (daily pages copy the previous page's background) + `stamp_page`.
- Launch: when `load_open_at_launch` is `Today`, after Home is ready call `open_today` once; `ClockUnset` falls back to Home with the notice `Set the Kindle clock first.`; `launch_clock::mark("today_open_end")`.

- [ ] **Step 1: Write failing tests** (`daily_tests.rs`, kit `Scribe` + `FolderStore`): `first_today_creates_folder_month_and_uses_page_one`, `second_today_same_day_reuses_the_page`, `today_after_rollover_appends_a_new_page_once` (now = next day → one new page, stamped), `month_rollover_creates_the_next_notebook_in_the_same_folder`, `ledger_re_resolves_renamed_and_recreates_deleted` (rename month notebook → still used; delete it → recreated, old day entries dropped), `corrupt_daily_ledger_fails_closed`, `preferences_open_at_launch_round_trips`, `home_layout_has_a_today_tile_that_does_not_overlap_new`.
- [ ] **Step 2: Run** `cargo test -p scribe-app --lib --locked daily` → FAIL.
- [ ] **Step 3: Implement** ledger, `resolve_today`, Home tile, command, prefs, settings rows, launch hook.
- [ ] **Step 4: Run** focused, then full suite → PASS; build clean.
- [ ] **Step 5: Commit** — `feat(daily): one page per day in a monthly Daily notebook`

---

### Task 7: Quick capture

**Files:**
- Create: `ignite/kindle/scribe-app/src/capture.rs`, `capture_tests.rs`
- Modify: `prefs.rs` (`SideButtonAction::Capture`), `nav.rs` (`SideButtonDispatch::Capture`), `settings_ui.rs` (third side-button row `Capture`, `PreferencesHit::Capture`), `home.rs` (`HomeHit::Capture`, `HomeLayout.capture_button`), `app_ui.rs` (`UiCommand::Capture`, `UiCommand::CaptureReturn`), `runtime.rs`, `lib.rs`

**Interfaces:**
- Consumes: Task 4, Task 5, Task 2 (`blank`).
- Produces:

```rust
pub const CAPTURE_DIR: &str = ".scribe-capture"; pub const CAPTURE_FILE: &str = "capture.json"; pub const INBOX_TITLE: &str = "Inbox";
#[derive(Default, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
pub struct CaptureLedgerV1 { pub v: u32, pub inbox: Option<String> }
pub struct CaptureLedger { /* open/put, prefix REFUSE CAPTURE: */ }
pub struct ReturnSlot { pub notebook_id: String, pub page_id: String }   // main-loop local, Option
pub fn resolve_inbox(ledger: &mut CaptureLedger, library: &LibraryRepository, create: &mut dyn FnMut() -> Result<String, String>) -> Result<(String, bool), String>;  // (inbox id, created)
pub fn next_capture_page(manifest: &PersistedNotebookManifestV1, created: bool) -> CapturePage;   // enum CapturePage { ReuseFirst, InsertAfter(String) }
pub fn should_discard_on_leave(page_stroke_count: usize, page_count: usize) -> bool;   // true when 0 strokes and page_count > 1
// nav.rs
SideButtonDispatch::Capture
pub fn dispatch_side_button(..) // SideButtonAction::Capture → Some(SideButtonDispatch::Capture), consuming the page like CreateLink
```

- Runtime: `UiCommand::Capture` (from the Home tile or the side-button dispatch while a page is open): remember `ReturnSlot` (only when coming from a page that is not the Inbox), resolve/create the Inbox (`Blank` template, top level), choose the page (`ReuseFirst` on a fresh notebook, else insert after last), `stamp_page` with `clock::stamp`, open it. Side button on an Inbox page → `UiCommand::CaptureReturn`: if the current page `should_discard_on_leave`, delete it through the normal delete path, then open the return slot (or go Home when empty). Navigating anywhere else from a capture page clears the slot and applies the same discard rule once.

- [ ] **Step 1: Write failing tests** (`capture_tests.rs` + `nav.rs` tests): `first_capture_creates_inbox_and_reuses_page_one`, `later_captures_append_stamped_pages`, `ledger_re_resolves_renamed_and_recreates_deleted`, `empty_capture_is_discarded_on_leave_but_the_only_page_stays`, `capture_dispatch_is_a_no_op_without_a_page_and_returns_from_inbox` (pure: `dispatch_side_button(Capture, held=false, ..)` → None; held → `Capture`; `should_discard_on_leave` cases), `settings_has_three_side_button_rows_without_overlap`, `home_layout_has_a_capture_tile`.
- [ ] **Step 2: Run** `cargo test -p scribe-app --lib --locked capture nav settings_ui` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** focused, full suite, build → PASS.
- [ ] **Step 5: Commit** — `feat(capture): side-button quick capture into an Inbox notebook`

---

### Task 8: Docs

**Files:**
- Modify: `README.md` (Features: three bullets), `docs/architecture.md` (`### Templates`, `### Reserved notebooks and text stamps` under Durable notebook storage), `docs/troubleshooting.md` (`## Templates, Daily page and Capture`: custom PNG not listed, `Set the Kindle clock first.`, where the Inbox went after a rename).

- [ ] **Step 1: Write the sections** from the spec's "Decided" lists and the shipped names only.
- [ ] **Step 2: Commit** — `docs: templates, daily page and quick capture`

---

### Task 9: Hardware gate (manual)

On the verified Scribe (FW 5.18.6):

- [ ] Time each built-in's first render from the log; second use hits the cache.
- [ ] Drop a PNG in `templates/custom/`, create a notebook from it, confirm letterboxing and that page 2 shares page 1's blob in the browser.
- [ ] `Today` on a device with the clock set: page headed with the local date matching the Kindle's own clock display; on a device whose clock is wrong: the notice, nothing created.
- [ ] Side-button capture from a page and back; empty capture gone; stamps readable in the browser.
- [ ] Record results in the release qualification notes.
