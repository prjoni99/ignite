# Templates, Daily page, Quick capture: design

Date: 2026-10-25
Status: draft, awaiting owner approval
Scope: three small Kindle-side features that make Ignite the thing you reach for every day

## Intent

The Amazon mirror (shipped) gets ink *out* of the Scribe. These three features
make Ignite worth writing *in*: page templates Amazon gates per notebook, a
daily page that is always one tap away, and a capture inbox for the thought
you need to write down right now. All three are files-first, need no server,
and change nothing in the sync protocol. A fourth idea from the same
conversation, searchable handwriting, depends on the vault's recognizer hook
(Scribe Liberation piece 2) and is not here.

The owner picked these; the ordering and the mechanics below are the author's
calls unless marked otherwise.

## Goals

- Pick a template when creating a notebook, or when inserting a page; ship a
  useful set; let the owner add their own by dropping a PNG on the device.
- Open Ignite and be on today's page without navigating; yesterday is one
  swipe back.
- From anywhere, get to a blank capture page in one action and back in one.
- Every result is visible in the browser companion and, later, the vault,
  without extra work there.

## Non-goals

- Changing an existing page's background (the protocol fixes a page's
  background at creation; a `SetPageBackground` operation is a protocol v2
  item, not this work).
- Template or capture UI in the browser companion.
- Handwriting recognition, search, or tasks.
- Cloud anything.

## Anti-goals

- Any added work on the pen path.
- A template library that costs storage per page beyond what a page already
  costs (a page background is a full-panel gray8 bitmap on the device today).
- A daily page that silently creates pages on a device whose clock is wrong.

## Constraints from the codebase

- A page background on the Kindle is a full-panel gray8 bitmap (1860 x 2480)
  stored per page; on sync it becomes a content-addressed blob
  (`BackgroundRef::Blob { sha256, byte_len, format }`), uploaded once and
  shared. The browser verifies and renders gray8 blobs already.
- A new page copies the previous page's background
  (`create_page_files_from_template`) **after** `scrub_chrome_band` rewrites
  the bottom 49 rows, so a copied page's bitmap (and blob hash) differs from
  the original unless those rows are already uniform. The top 130 px sit
  under the chrome bar.
- Every page stores its own full `background.raw` on the device (about
  4.6 MB); only sync blobs and the server dedupe by hash. Sharing storage
  between pages on the device would be a store-format change and is out of
  scope.
- The app launches into Home today (`runtime.rs`, "F11: launch into Home").
  Inside a folder, `New` skips the chooser and goes straight to title entry.
- Insert page is a button in the page-menu dialog (`INSERT PAGE AFTER`), not
  a chrome control; dialog buttons have no long-press.
- The page-delete path refuses to delete a notebook's only page; every new
  notebook already starts with `page-1`.
- No code converts the clock to local time; `scribe-app` has no `libc` or
  date dependency today. The pure-Rust `png` crate is not yet a dependency
  (its `miniz_oxide` backend already is, via `flate2`).
- Notebook creation takes a `background: &[u8]` (`create_notebook_durable`),
  today always `paper_background` (white).
- Typed text objects (`SyncText`, face `default-sans-v1`) are painted on the
  Kindle (`text_paint.rs`) from `texts.jsonl` and rendered in the browser, but
  the Kindle does not author them yet; `SyncOperationV1::AddText` exists.
- The side button is a preference (`SideButtonAction::{NavigationUndo,
  CreateLink}`) dispatched on pen-down over a page; it has no meaning on Home.
- Home shows a `New` tile that opens a chooser (notebook or folder).
- Replica-local preferences live in `preferences.json` (`v: 1`, serde
  defaults for new fields).
- Pure-Rust crates only on the device; the ARMv7 build is cross-compiled.

---

## 1. Templates

### What it is

A template is a page background. Ignite ships a parametric set rendered on
the device, and accepts custom bitmaps the owner drops on the Kindle.

### Decided

- **Parametric built-ins, rendered on device, cached once.** A new crate
  `ignite/core/page-templates` renders a `TemplateSpec` to a gray8 panel
  bitmap: `Blank`, `Lined { spacing_px, margin_px }`, `DotGrid { pitch_px }`,
  `Graph { pitch_px }`, `Cornell`, `Daily` (date band at top, lined below),
  `Weekly` (seven rows), `Isometric { pitch_px }`, `MusicStaff`. Ink value
  for lines is a light gray (author's default `0xC0`), dots `0xA0`. Rendered
  bitmaps are cached under `/mnt/us/scribe-data/templates/cache/<sha256>.gray8`
  and reused by hash; the first use of each template costs one render
  (milliseconds, not on the pen path). Every built-in keeps the top 130 px
  (under the chrome bar) and the bottom 57 px plain white, so the chrome
  scrub on page copy changes nothing and every page of a notebook shares one
  blob hash. Custom PNGs are letterboxed into the same safe area.
- **Custom templates** are PNG files the owner copies to
  `/mnt/us/scribe-data/templates/custom/` over USB (name = file name without
  extension). Decoded with the pure-Rust `png` crate, converted to grayscale,
  scaled to fit 1860 x 2480 with white letterboxing, cached like built-ins.
  Unreadable files are listed as such in the picker and skipped.
- **Where you pick one.** The notebook title-entry dialog (reached from the
  `New` chooser at top level and directly inside a folder) gains a template
  row: thumbnails (built-ins then custom) with the selected one outlined; the
  choice applies to the notebook's first page, and therefore to every page
  added afterwards (copy-previous-page behaviour). The page-menu dialog gains
  a second button `INSERT PAGE WITH TEMPLATE` next to `INSERT PAGE AFTER`,
  which opens the same picker and creates the new page with that background;
  `INSERT PAGE AFTER` keeps copying the current page.
- **Last-used template** is remembered in `preferences.json`
  (`template: <id>`), default `Blank`, so the chooser opens on it.
- **Thumbnails** are the bitmap downscaled 1:8 at render time and cached
  beside it (`<sha256>.thumb.gray8`).
- **Sync and browser:** nothing new; the background blob flows as today.

### Left to the builder

- Exact line spacings and margins (pick ones that look right at the panel's
  300 ppi; the owner can override in a later pass).
- Thumbnail grid geometry in the chooser.

### Done looks like

Create a notebook with Dot Grid, write on it, add a page, open it in the
browser: all pages dotted. Drop `grocery.png` in `templates/custom/`, create a
notebook from it: the list appears letterboxed. The template cache holds one
bitmap per distinct template; pages keep storing their own background as
today.

---

## 2. Daily page

### What it is

A journal that is always open to today. One page per day, stamped with the
date, in a monthly notebook inside a `Daily` folder.

### Decided

- **One notebook per month**, titled `Daily 2026-10`, created on first use of
  that month with the `Daily` template, filed in a folder titled `Daily`
  (created on first use; the folder id is recorded in a replica-local
  `daily.json`, like the mirror's folders, so a rename keeps working).
- **One page per day**, created the first time the day is opened, appended
  after the latest page; a freshly created monthly notebook's `page-1` *is*
  that day's page (no delete needed). The page carries a **date stamp as a typed text
  object** (`SyncText`, `default-sans-v1`) at the top: `Saturday 25 October
  2026`. Author's call: text objects survive into the browser and the vault
  as real text, which a baked-in bitmap would not; this needs the Kindle to
  author `AddText` (a `prepare_add_text` capture, same ticket discipline as
  links), which is new Kindle code but no protocol change. Stamps sit below
  the 130 px chrome bar.
- **Which page is today** is recorded in `daily.json` (`date → notebook id,
  page id`); the stamp is for humans and the vault, the ledger is for the
  app. Opening "today" when the ledger has no entry creates the page.
- **How you get there.** A `Today` tile on Home next to `New`; a Preferences
  choice `Open at launch: Home | Today` (default `Home`, today's behaviour;
  `Today` binds the daily page at startup and is measured by `launch_clock`);
  and inside a daily notebook, the last page is today, previous pages are
  previous days, so the existing swipe is "yesterday".
- **Clock and time zone.** Author's call: local time comes from libc
  `localtime_r` (add `libc` to `scribe-app`), which reads the Kindle's
  `/etc/localtime`; the hardware gate checks that the result matches the
  Kindle's own clock display. If it does not, a `clock_offset_minutes`
  preference (default 0) is the fallback, added only if the gate fails. If
  the device date is before 2025-01-01 (an unsynced clock), `Today` shows
  `Set the Kindle clock first.` and creates nothing.
- **Month rollover** happens whenever `Today` is used, not on a timer.

### Left to the builder

- The stamp's position and size (top band of the `Daily` template is
  reserved for it; author's default 36 pt equivalent, left-aligned).
- Whether `Today` on Home is a tile or a chrome button; a tile is the
  author's default.

### Done looks like

Tap `Today`: a page headed with today's date, on the Daily template, in
`Daily 2026-10` inside the `Daily` folder. Tomorrow, `Today` appends a new
page; swipe back shows today's. In the browser, the date is selectable
text.

---

## 3. Quick capture

### What it is

A reserved `Inbox` notebook and a one-action way in and out of a fresh page
in it, from Home or from the middle of writing something else.

### Decided

- **`Inbox` notebook**, created on first capture, top level (no folder),
  `Blank` template, id recorded in `capture.json` (replica-local); its
  `page-1` is the first capture page. Every later capture appends a page.
  Each capture page is stamped with a typed timestamp `Sat 25 Oct 14:32`
  (same text-authoring path as the daily stamp, placed below the chrome
  bar).
- **From Home:** a `Capture` tile next to `Today`.
- **From a page:** a new side-button action `Capture` (third
  `SideButtonAction` variant in Preferences). Pressing the side button and
  touching the page jumps to a fresh Inbox page; doing it again on the Inbox
  page returns to the notebook and page you came from (a one-deep return
  slot in the runtime, cleared when you navigate normally). The existing
  actions keep their current meaning.
- **Leaving a capture page empty** (no strokes when you leave it) deletes
  that page again through the normal page-delete path when it is not the
  notebook's only page (the only page stays, blank, and is reused by the next
  capture), so the Inbox never fills with blanks. Author's call.
- **Vault and browser:** nothing new; the Inbox is an ordinary notebook, so
  piece 2's vault will emit one file per capture page with its timestamp
  text.

### Left to the builder

- Timestamp format detail and placement; keep it one line.

### Done looks like

Writing in a meeting notebook, side button plus touch: a blank Inbox page
with a timestamp. Jot, side button plus touch: back on the meeting page where
you were. The Inbox shows the jot; the empty capture you abandoned is gone.

---

## Cross-cutting

- **Shared mechanism: Kindle-authored text.** One `text_author` module adds
  `prepare_add_text` to outbound sync and writes `texts.jsonl`; both stamps
  use it. Text objects are display-only on the Kindle today and stay that way
  (no editing on the device).
- **Reserved notebooks** (`Inbox`, monthly `Daily …`) are ordinary notebooks:
  the owner may rename, move or delete them; the ledgers (`daily.json`,
  `capture.json`) re-resolve by id and recreate when the target is gone.
- **Preferences** gains `template`, `open_at_launch`, and the `Capture`
  side-button variant; `v` stays 1.
- **Build order:** 1 templates (the daily template needs it), 2 Kindle text
  authoring (shared), 3 daily page, 4 quick capture. Each ships on its own.
- **Testing:** the template crate gets golden hashes per spec; chooser
  layout and picker hits get unit tests like the existing settings tests;
  `daily.json`/`capture.json` ledgers get open/corrupt/re-resolve tests;
  text authoring gets a capture-exactness test mirroring the link tests;
  hardware gate: render time of each template, thumbnails, side-button
  round trip.
- **Docs:** README feature bullets, `docs/architecture.md` (templates cache,
  reserved notebooks, text authoring), troubleshooting (custom PNG not
  showing, clock guard).

## Review log

2026-10-25: builder check against the codebase. Three questions (time
zone source, launch default, device storage of templates) and five
corrections (chrome scrub on page copy, no long-press on dialog buttons,
folder `New` skips the chooser, only-page delete refusal, `png` is a new
dependency) resolved in this revision; see "Author's call" markers.
