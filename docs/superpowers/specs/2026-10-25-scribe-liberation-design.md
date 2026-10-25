# Scribe Liberation: design

Date: 2026-10-25
Status: draft, awaiting owner approval
Scope: four features that together make Ignite the way ink leaves a Kindle Scribe

## Intent

Amazon limits what a Scribe notebook can become: it stays inside Amazon's app,
and the only exits are "convert to text" and email. Ignite already owns the
other half of the problem (a native notebook app, a canonical stroke protocol,
a self-hosted sync server, a browser replica, and a read-only decoder for
Amazon's own notebook format). This design turns Ignite Server into the exit:
every notebook on the device, Amazon's or Ignite's, becomes plain files on a
machine the owner controls, and the same channel carries documents back onto
the device.

The owner's principles apply throughout: files first, no lock-in, self-hosted,
no cloud dependency required for any feature, no subscriptions.

## The four pieces, in build order

1. **Amazon mirror**: continuous one-way export of stock Scribe notebooks into a read-only `Amazon` folder of the device's library.
2. **Vault**: Ignite Server materializes every library as a folder of SVG, PNG, PDF, JSON and Markdown files, atomically, on every change.
3. **Print to Scribe**: a server inbox folder and web upload turn a PDF into an annotatable Ignite notebook that syncs to the Kindle.
4. **Live ink**: an opt-in mode that streams strokes to the server as they are written, rendered in the browser within a few hundred milliseconds.

Each piece ships on its own. Pieces 3 and 4 depend on nothing in 1 or 2 except
that 3's annotated output is more useful once 2 exists.

## Goals

- Ink written anywhere on the Scribe (Amazon app or Ignite) ends up as portable files without the owner touching the device.
- The vault is consumable by any file-syncing tool (Syncthing, Nextcloud, Obsidian, iCloud Drive, git) with no Ignite-specific client.
- Dropping a PDF into a folder on the server is enough to annotate it on the Scribe.
- Nothing in this design writes to Amazon's storage, ever.

## Non-goals

- Two-way editing of Amazon notebooks. The mirror is read-only; edits happen in Ignite copies.
- Editing vault files and having changes flow back. The vault is derived output.
- Built-in handwriting recognition. It is a pluggable hook (see Vault).
- Hosted connectors (OneNote, Notion, Google Drive, Dropbox). Deferred; the vault makes them a later, separable project.
- Rendering web pages to the Scribe. v1 accepts PDF only.
- Exposing the plain-HTTP server to the public Internet. Off-LAN use goes through Tailscale, which this design does not change.

## Anti-goals (failure even if it works)

- Any path that opens an Amazon notebook file for writing, creates a journal next to it, or holds it locked.
- A vault writer that leaves a half-written file visible to a sync tool.
- Live ink adding latency or risk to the pen path when the mode is off.
- A deleted notebook whose vault folder is hard-deleted.

## Constraints from the codebase

- `nbk-reader` decodes only the materialized `NBK_MATERIALIZED` state and refuses notebooks whose action log is ahead of it (`NbkError::PendingActions`). The mirror inherits this: a notebook Amazon has not flushed is skipped this round, never guessed.
- `nbk_import` identifies Amazon notebooks by catalog UUID (`/mnt/us/.notebooks/<uuid>/`), never by title or mtime. The mirror keeps that identity.
- Imports run only at idle safe points, never on the pen path. The mirror obeys the same rule.
- Sync is versioned `SyncOperationV1` envelopes over `/v1` on `scribe-sync-server`; pages carry a `BackgroundRef` that is `Blank` or a content-addressed `Blob` uploaded via `PUT /v1/blobs/{sha256}`. Print to Scribe uses exactly these.
- Server-side acceptance of destructive lineage replacement is off by default; nothing here needs it on.
- Panel geometry is 1860 x 2480 (`FbGeometry` at runtime; the `--diagnose-fb` Gate B check in `scribe-app`). The Kindle **refuses** inbound blob backgrounds whose dimensions differ from the panel (`sync_materialize.rs`), so rasterizing at exactly that size is mandatory.
- A Kindle runtime has exactly **one** library (one `--data-dir`, one `sync-state.json`, one `library_id`). Libraries have no name anywhere in the protocol; the control plane names devices.
- There is no shared Rust rasterizer: the Kindle rasterizes in `scribe-app` (`render.rs`, `profile.rs`, `pressure_curve.rs`), the browser in TypeScript (`ignite/web/src/rendering/brush.ts`), pinned to the Rust-generated `brush-v1.json` fixture. `scribe-web-wasm` does encoding and erase projection, not pixels.
- `POST /v1/.../operations` caps a batch at 100 envelopes, and a notebook must always have at least one page.
- `scribe-sync-server` today never authors notebook content of its own; Print to Scribe changes that (see piece 3).
- `scribe-server` already has an in-process exporter pattern (`export.rs`) that reads the server's own `/v1` GET router with no HTTP and no credential. The vault writer reuses that pattern.
- On-disk server paths stay `/etc/scribe-server` and `/var/lib/scribe-server`.

---

## 1. Amazon mirror

### What it is

A reserved folder named `Amazon` in the device's single library, created with
the existing `CreateFolder` / `SetNotebookFolder` operations, that holds a
read-only copy of every stock Scribe notebook, kept current by re-snapshotting
whenever a source changes. (Author's call after review: a second library would
mean a second repository, sync dir, outbox and server bootstrap, most of a
project for no visible gain; a folder gives the same experience.) It replaces the one-shot "Import
all" as the default relationship with Amazon notebooks. "Import all" stays as
the way to get an editable Ignite copy.

### Decided

- **Separate notebooks in a reserved folder, not in-place re-import.** The existing copy-once receipts
  stay as they are. Mirror state lives in its own ledger
  (`<data-dir>/.scribe-mirror/ledger.json`, beside the existing `.scribe-import/`) keyed by the catalog UUID (`origin_kind: amazon_nbk` is recorded in each mirror's `mirror-origin.json` marker), recording
  the source stamp (size + mtime of `nbk` and `actions.log`) and the content
  hash of the last decoded snapshot.
- **Read-only by convention, no protocol change.** On the Kindle each mirror
  notebook carries an immutable `mirror-origin.json` marker (like
  `import-origin.json`); pen input on a mirror page is refused with a one-line
  hint, and "Copy to Ignite" runs the existing import path to make an editable
  copy outside the folder. The browser companion treats any notebook inside a
  folder named `Amazon` as read-only (view only, banner explaining why).
  Author's call: a protocol-level attribute would change canonical bytes and
  need a version bump; the convention is enough for v1 and can be hardened
  later.
- **Replace wholesale per notebook.** When a source's decoded content hash
  changes, the mirror notebook is rebuilt: new pages and strokes emitted as
  ordinary sync operations in the order create new pages, add strokes, then
  delete the old pages (a notebook must always keep at least one page), in
  batches of at most 100 envelopes with the same journal backpressure as
  `nbk_import`. The server and browser replica see a normal notebook. Rename
  follows the catalog title. A source that disappears from the catalog moves
  its mirror notebook to a `Removed from Amazon` folder rather than deleting
  it.
- **Cadence and where it runs.** A *scan* (stat the catalog and each source
  directory) runs at app launch, after each successful sync upload cycle, and
  on an idle timer (default 15 minutes). A *rebuild* (decode + emit ops) runs
  only at the existing idle safe point and only while no page is open (Home or
  Tools view), so it can never stall the pen path; a pending rebuild waits for
  the user to leave the page. No new background thread. Author's call: this is
  the smallest change that keeps the anti-goal.
- **Skipped sources are retried, not failed.** `PendingActions`, `SourceBusy`
  and `SourceChanged` leave the ledger untouched so the next rescan tries again.
  `Unsupported` and `Container` errors are recorded with the stamp and reason
  in the mirror ledger so they are not re-attempted until the source changes.
  The Tools view gains a per-source status list (new work: today only the
  current run's failures are shown, nothing is persisted).
- **Opt-in toggle** in Tools, default on once the user has enabled sync; off
  when sync is off (a mirror that never leaves the device is just a slower
  copy).

### Left to the builder

- Whether mirror rebuilds reuse stable page ids across versions (nicer history
  on the server) or mint fresh ones (simpler). Either is acceptable; the server
  history is not user-facing.
- Batch sizes and durable-journal backpressure, matching `nbk_import`.

### Done looks like

Write in Amazon's notebook app, lock the device, open Ignite later: within one
rescan the browser companion shows the notebook under `Amazon`, strokes match,
and `/mnt/us/.notebooks` has no new files, journals or mtime changes.

---

## 2. Vault

### What it is

A directory tree under `/var/lib/scribe-server/vault/` that Ignite Server keeps
equal to the current state of every library, as plain files.

```
vault/
  <library name>/                 (see Naming)
    <notebook title> [<short id>]/
      page-001.svg
      page-001.png
      ...
      notebook.pdf
      strokes.json
      notebook.md
    .trash/
      <notebook title> [<short id>] <deleted-at>/...
  inbox/                 (piece 3)
```

### Decided

- **Derived, regenerable, one-way.** The sync store is the source of truth. A
  `vault rebuild` admin action wipes and regenerates. Nothing reads the vault
  back.
- **Atomic per notebook.** The writer renders into a sibling temp directory
  and renames it over the notebook folder (on the same filesystem). A watcher
  never sees a partial page set.
- **Library naming.** Libraries have no name in the protocol. The server
  config gains `[vault.names]`, a map from library id to folder name, editable
  from the browser companion's server settings (stored server-side through the
  control plane). Unnamed libraries use `library-<short id>`. Author's call;
  a protocol-level library name is not worth a version bump.
- **Notebook naming.** Titles are sanitized to a safe cross-platform subset (no path
  separators, control characters, reserved Windows names, trailing dots or
  spaces; length capped). The short id suffix (first 8 chars of the notebook
  id) makes names unique and stable across renames; a rename renames the
  folder.
- **Delete moves to `.trash`**, with a timestamp suffix. Trash is never
  emptied automatically.
- **Formats.**
  - `page-NNN.svg`: strokes as `<path>` elements with per-segment widths from
    the render profile's pressure curve, viewBox 1860 x 2480, background
    referenced as an embedded PNG when the page has a blob background.
  - `page-NNN.png`: 8-bit grayscale raster at panel resolution. There is no
    shared rasterizer today; `notebook-render` is a third implementation of
    the brush, pinned to the same `brush-v1.json` fixture the browser uses,
    with the browser's contract: logical stroke widths equal, antialiasing may
    differ. The Kindle rasterizer is **not** moved (pen-path code, hardware
    re-qualification). Author's call.
  - Positioned text (`SyncText`) is rendered in SVG, PNG and PDF using the
    fonts already bundled under `ignite/assets/` (SIL OFL), matching the
    browser's layout for the `default-sans-v1` face. Page links
    (`SyncPageLink`) are emitted as `<a>` elements in SVG and omitted from PNG
    and PDF in v1. Author's call: dropping typed annotations from
    `notebook.pdf` would silently lose content.
  - `notebook.pdf`: one page per notebook page, vector strokes over the
    background image.
  - `strokes.json`: the canonical `SyncSnapshotV1` subset for this notebook
    (lossless; this is the file a future importer reads).
  - `notebook.md`: front matter (title, ids, page count, last modified), then
    one `## Page N` section per page with an image link to the PNG and, when a
    recognizer has run, the recognized text.
- **Rendering lives in a new crate** `ignite/core/notebook-render` (SVG, PNG,
  PDF from `SyncSnapshotV1` + blobs). It may later replace the TypeScript
  brush via the WASM bridge; that is not part of this work.
- **Trigger.** The writer runs in `scribe-server` after each accepted `/v1`
  envelope batch, debounced (default 2 s) per notebook, plus the admin rebuild.
- **Recognizer hook.** Config key `recognizer_command` in
  `/etc/scribe-server`. When set, after a notebook is rendered the server runs
  the command once per changed page with the PNG path as argument and reads
  UTF-8 text from stdout (bounded size, bounded wall time, non-zero exit means
  "no text this round"). Results are cached by page content hash in
  `/var/lib/scribe-server/recog/`. Unset by default; v1 ships no model.
- **Browser companion** gets a "Vault" line in server settings showing the
  path, last write time, and the rebuild button (admin, loopback or paired
  session per existing control-plane rules).

### Left to the builder

- PDF library choice (pure-Rust preferred; no system dependency on the Linux
  bundle or the GHCR image).
- Whether notebooks under the `Amazon` folder get a `source: amazon` line in
  `notebook.md` front matter. Recommended yes, cheap.
- Exact debounce and bounded-size numbers.

### Done looks like

Point Syncthing at `vault/`, write a page on the Scribe, and the SVG, PNG, PDF
and Markdown appear on a laptop with no Ignite software on it. Renaming the
notebook renames the folder; deleting it moves the folder to `.trash`.

---

## 3. Print to Scribe

### What it is

Two front doors, one factory. Drop a PDF into `vault/inbox/` or upload one in
the browser companion; the server builds an Ignite notebook whose pages are the
PDF's pages as backgrounds, and it syncs to the Kindle like any notebook.

### Decided

- **Input: PDF only** in v1. Other types are left in place and listed as
  "unsupported" in the inbox status. Web pages come later via print-to-PDF.
- **Target library** is chosen per upload in the browser; the inbox folder
  uses `inbox.default_library` from server config and supports one subfolder
  level named after a vault library name or id to override it
  (`inbox/<library name>/x.pdf`).
- **The server becomes an authoring replica.** This is a principle change:
  `scribe-sync-server` has never written notebook content of its own. It gets
  a durable `ReplicaId` and a persisted, strictly increasing `client_seq`,
  stored inside the ledger store so a backup/restore of the server carries it.
  Portable backups contain no replica identities, so restoring one into a new
  server mints a fresh replica at genesis; the old server's sequence state is
  simply not reused. All inbox operations go through `Store::submit` like any
  replica's, in-process via `http::route`. Author's call on the mechanics; the
  principle change follows directly from the owner asking for Print to Scribe.
- **Rasterization** to 8-bit grayscale at exactly 1860 x 2480 (the Kindle
  refuses other sizes), fit-to-page with letterboxing, one background blob per page, uploaded via the existing blob
  endpoint and referenced as `BackgroundRef::Blob`. Page count is capped
  (default 200); larger PDFs are refused with a clear status.
- **Notebook title** is the PDF filename without extension, sanitized the same
  way as vault names. The first page uses `CreateNotebook`, the rest
  `CreatePage`, in batches of at most 100 envelopes, all through the server's
  normal `/v1` path in-process, so the Kindle and browser see ordinary
  operations.
- **Inbox lifecycle.** A processed file moves to `inbox/.done/`, a refused one
  to `inbox/.failed/` with a sibling `.txt` reason. Files are only picked up
  once their size has been stable for a few seconds (partial copies).
- **Annotated round trip** needs no new code: the resulting notebook is a
  normal notebook, so the vault emits `notebook.pdf` with ink over the original
  pages.

### Left to the builder

- PDF rasterizer choice (pure-Rust preferred; same constraint as the vault's
  PDF writer). If no pure-Rust rasterizer is adequate, a bundled `pdftoppm`
  is acceptable in the Linux bundle and Docker image, documented as a
  dependency.
- Whether the inbox watcher is inotify-based or a short poll. Poll is fine.

### Done looks like

Copy a 30-page PDF into `inbox/`, wait for the next sync, open it on the
Scribe, write on page 12, and find `notebook.pdf` in the vault with that ink
on page 12.

---

## 4. Live ink

### What it is

A "Live" toggle in Ignite. While on, each committed stroke is also sent to the
server immediately over a persistent connection, and the browser companion
renders it as it arrives.

### Decided

- **Off by default, per session.** Turning the app off or sleeping turns Live
  off. It is a presentation mode, not a sync mode.
- **No change to durable sync.** Live frames are advisory: the same strokes
  still flow through the normal outbox and envelopes. The browser treats live
  frames as a transient overlay on the current page, reconciled when the real
  operation arrives (matching by stroke id). A missed frame is harmless.
- **Transport.** Kindle to server: one long-lived HTTP connection posting
  newline-delimited JSON frames (`{notebook_id, page_id, stroke}` using the
  existing `SyncStroke` shape). Server to browser: Server-Sent Events on a
  `/control/v1/live/{library}` route, paired-session authorization as for
  other control routes. No WebSocket dependency on the Kindle side.
- **Pen-path isolation.** Frames are enqueued from the same idle safe point
  where strokes are committed to the journal, never from the sampling or
  rasterizing path. The queue is bounded; when full, frames are dropped, not
  blocked. With Live off the code path is a single boolean check.
- **Latency target:** under 300 ms from pen-up to browser paint on a LAN.
  Measured during hardware qualification; not a release blocker.
- **Browser view.** A "Live" indicator on the page view and a full-screen
  mode suited to screen sharing.

### Left to the builder

- Frame batching interval (tens of milliseconds) and queue depth.
- Whether partial strokes (pen still down) are ever streamed. v1: pen-up only.

### Done looks like

Toggle Live, share the browser tab in a video call, write on the Scribe, and
the far end sees the ink within a beat. Toggle it off and battery drain
returns to normal with no code on the pen path.

---

## Cross-cutting

- **Security.** Vault and inbox are local filesystem on the server; exposure
  is whatever the owner points at them. Recognizer commands run as the server
  user with the PNG path as the only argument. Live and inbox routes follow
  the existing control-plane rules (loopback or paired session). Nothing here
  widens the plain-HTTP exposure.
- **Packaging.** New server features ship in the Linux bundle and the GHCR
  image together. Kindle-side changes (mirror, Live toggle) ship in the
  installer. Each piece is a separate release train entry.
- **Testing.** Mirror: fixture notebooks under the existing
  `nbk-import-samples` feature, plus a never-writes assertion on the source
  tree and a never-rebuilds-with-a-page-open test. Vault: golden files for SVG
  and Markdown, PNG checked against the `brush-v1.json` fixture (logical
  widths, not pixel identity), atomicity tested by interrupting a write. Print: round
  trip a generated PDF. Live: queue-bound and off-path tests on the Kindle
  side; SSE ordering in the server.
- **Docs.** `docs/self-hosting.md` gains Vault, Inbox and Recognizer sections;
  `docs/architecture.md` gains the new crate and routes.

## Review log

2026-10-25: builder check (fresh reviewer, read the doc and the codebase).
Seven questions and five factual corrections, all resolved in this revision
and marked "Author's call" where the author settled them. No open questions
for the owner.
