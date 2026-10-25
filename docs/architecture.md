# Ignite architecture (contributor overview)

Ignite is a local-first handwriting notebook for a prepared Kindle Scribe, with
optional self-hosted sync and **ignite-web** (`ignite/web/`) as the browser replica.

## Kindle native runtime

| Crate | Role |
|-------|------|
| `scribe-app` | Main notebook UI/runtime: ink, pages, library, sync client hooks |
| `scribe-lifecycle` | Process supervisor / relaunch helper used by the host script |
| `kindle-platform` | Framebuffer, input, timing, and device glue |
| `ink-probe` | Hardware laboratory binary (not the customer app) |

The Library entry launches via `scribe-lifecycle-host.sh` (appmgrd handler
`com.markrai.ignite`), which starts `scribe-lifecycle` → `scribe-app` with
explicit `--data-dir`, `--sync-dir`, and `--device-dir` paths under `/mnt/us`.

Pre-0.9.11 installs used handler `com.markrai.scribe`. The installer migrates
that registration to `com.markrai.ignite` without moving binaries or notebook
data. Sync discovery identity `_scribe._tcp` and storage under
`/mnt/us/scribe-data` are unchanged.

## Rendering and editing

Stylus samples are processed into strokes, rasterized for the e-ink panel, and
committed into durable page state. UI chrome (library, tools, sleep) is separate
from page ink. Polarity / grayscale handling differs across Scribe panel modes;
see the compatibility matrix.

## Durable notebook storage

Notebooks live under `/mnt/us/scribe-data` (customer data). Sync metadata and
device state live under `/mnt/us/scribe-app/state/`. Installer upgrade/uninstall
paths are designed to preserve notebooks unless the user deliberately deletes
them.

Shared types and reducers live in:

- `notebook-core` — notebook operations and geometry helpers
- `sync-protocol` — canonical envelopes and hashing
- `sync-engine` — apply / reduce / testkit fixtures

### Amazon mirror

`scribe-app`'s `mirror` module continuously mirrors stock Kindle notebooks,
read-only, into the Ignite library so they reach the sync server and the
browser companion.

- Source: `/mnt/us/.notebooks/<uuid>/` plus the catalog
  `/var/local/ksdk.content.db`, read-only; nothing there is written, locked or
  renamed. Identity is the catalog UUID.
- Mirrors live in the device's single library, in a folder titled `Amazon`;
  each carries an immutable `mirror-origin.json` marker in its notebook
  directory. A source deleted on the Kindle moves its mirror to `Removed from
  Amazon` (never deleted); if it reappears it is rebuilt and filed under
  `Amazon`. Either folder may be renamed; the folder id is what matters.
- Ledger: `<data-dir>/.scribe-mirror/ledger.json` (beside `.scribe-import/`),
  one entry per source UUID: state `current`, `rebuilding`, `removed` or
  `unsupported`, source stamp (size + mtime of `nbk` and `actions.log`) and
  content hash. A corrupt ledger fails closed: the mirror pauses and nothing
  is created or removed. A missing notebooks root or catalog likewise pauses
  the mirror; a source the catalog still lists but whose directory or `nbk`
  is missing is skipped, not removed. Only a source the catalog drops (or
  marks deleted) moves to `Removed from Amazon`.
- Cadence: a stat-only scan runs at launch, after each completed sync upload
  batch and every 15 minutes. A rebuild (decode + ordinary sync operations)
  runs one notebook per loop iteration. A scan runs only while Home or Tools is
  showing and the pen is idle; a rebuild only while Home or Tools is showing,
  never with a page open. Never while sync is off or unlinked.
- Rebuild order: new pages and strokes first, old pages deleted after, so a
  notebook never drops below one page. Strokes flush to the journal in batches
  (same backpressure as "Import Kindle notebooks"). A crash mid-rebuild is
  cleaned up at next launch (partial mirror removed via the synced delete path,
  recreated by the next scan); never more than one mirror per source.
- Read-only: on the Kindle, an open mirror refuses pen/eraser, undo, page
  add/move/delete, link add/remove, lasso moves and rename with a
  `Read-only mirror` notice (`This notebook mirrors Amazon and is read-only.
  Use Copy to Ignite from Home.`). A mirror's Home card menu offers only
  `COPY TO IGNITE` and `DELETE` (a deleted mirror is recreated at the
  source's next change). In the browser companion, a notebook in a folder
  titled exactly `Amazon` shows `Mirrored from Amazon. Read-only here; write
  in Ignite or copy it on the Kindle.` and offers no Write/Type tools or page
  insert/delete. Renaming the `Amazon` folder or moving a notebook out of it
  (browser or Kindle folder commands) lifts the browser's read-only
  treatment; ink or text added afterwards is discarded at the next rebuild.
- Copy to Ignite: the Home card menu runs the existing one-way import to make
  an editable copy; a source is copied at most once (an `Already copied`
  notice afterwards).
- Crash recovery runs once per process at the first safe point where sync is
  linked, not at launch: with sync off or faulted the partial's delete would
  be local only and the server would keep it. Until it runs, a scan sees the
  `rebuilding` entry and rebuilds the partial in place (never a second mirror).
- Restore from Server replaces the library with the server's copy. The
  `Amazon` folder and the mirrored notebooks come back, but neither the ledger
  nor the `mirror-origin.json` markers do (markers are not synced), so the
  restored copies are ordinary, editable notebooks. The next scan recreates
  every mirror from the Kindle's sources and files them into the existing
  folder titled `Amazon` (an unrecorded folder with that exact title is
  reused, never duplicated). Restored copies are deliberately *not* adopted:
  a title plus page count also matches a user's own notebook, and a wrong
  claim would make it read-only and overwrite its ink at the next rebuild.
  The user may delete the unmarked restored copies; the marked ones are the
  mirrors.
- Setting: Preferences → `AMAZON MIRROR` (`mirror_amazon` in replica-local
  `preferences.json`, default on). Tools shows status and `RESCAN NOW`.

### Templates

Nine built-in page backgrounds are rendered on the device from parameters:
`blank`, `lined`, `dot_grid`, `graph`, `cornell`, `daily`, `weekly`,
`isometric`, `music_staff`.

- Custom templates are PNG files the owner copies over USB to
  `<library root>/templates/custom/` (the library root is `--data-dir`,
  default `/mnt/us/scribe-data`). Any PNG colour type works; it is converted to
  grayscale, letterboxed into the page's safe area and listed by its file name
  without extension. Unreadable PNGs are listed as unreadable and skipped.
- Safe rows: the top 130 px under the chrome bar and the bottom 57 px stay
  white.
- Cache: one bitmap per template under `templates/cache/` (`<sha256>.gray8`, a
  thumbnail beside it, and `index.json`). A changed PNG, or an app update that
  changes a built-in, re-renders.
- Each page still stores its own background as before; the cache is one
  bitmap per template, not per page.
- Picker: the notebook title dialog (from `+ NEW` on Home, top level or inside
  a folder) is followed by a paged template picker. The choice applies to the
  first page and, because new pages copy the previous page's background, to
  every later page. The page menu's `INSERT PAGE WITH TEMPLATE` (next to
  `INSERT PAGE AFTER`) opens the same picker for the new page.
- The last-used template is remembered (`preferences.json` key `template`,
  default `blank`) and preselected, marked with ` *`.
- A load failure shows `That template couldn't be loaded.` and creates nothing.
- Sync and the browser need nothing new: a template is an ordinary page
  background blob.

### Reserved notebooks and text stamps

Two notebooks are created and tracked by the app; both are ordinary notebooks
that sync normally.

- Daily: a folder titled `Daily` holds one notebook per month titled
  `Daily 2026-10` (year-month). `TODAY` on Home opens today's page; each day's
  page is appended after the previous day's, and a new month's first page is
  that day's page. Dates come from the Kindle's clock and local time zone; a
  clock reading before 2025 shows `Clock not set` and creates nothing.
- Each day page is stamped at the top with the date as typed text (`Sunday 25
  October 2026`), so it is real text in the browser companion.
- Ledger: `<library root>/.scribe-daily/daily.json` tracks the folder, month
  notebooks and day pages by id. Either may be renamed. A deleted month
  notebook is recreated at the next `TODAY`, a deleted `Daily` folder at the
  next month. The folder and the day-page ledger are replica-local. A corrupt
  `daily.json` shows an error and creates nothing.
- Inbox: `CAPTURE` on Home, or the `Capture` side-button action, opens a fresh
  page in a top-level notebook titled `Inbox` (created on first use), stamped
  with the time (`Sun 25 Oct 17:40`; skipped when the clock is unset).
  Tracked by id in `<library root>/.scribe-capture/capture.json`.
- Gesture: with the side-button action set to `Capture`, pressing the pen's side
  button and touching the page jumps to a fresh Inbox page. The same gesture on
  that Inbox page returns to the notebook and page you came from, or to Home
  when the capture started there.
- Leaving a capture page discards it if nothing was written, it has no links
  and no text beyond the stamp, and it is not the Inbox's only page. Other
  Inbox pages are never touched.
- Kindle-authored text: the two stamps are the first text objects authored on
  the device. They sync through the existing text operation and are
  display-only on the Kindle (no editing there).
- Preferences: `OPEN AT LAUNCH` (`Home` default, or `Today`; key
  `open_at_launch`) and `SIDE BUTTON` (`Nav + undo`, `New link`, `Capture`).

## Sync and operation envelopes

Outbound/inbound sync uses versioned operation envelopes. A self-hosted
`scribe-sync-server` can accept a replica’s history over HTTP on a trusted LAN.
Server-side acceptance of destructive lineage-replacement operations is **off
by default** and must not be enabled casually.

Pairing / discovery helpers live in `scribe-server` + `control-protocol`.

## ignite-web

`ignite/web/` is the browser notebook replica (Vite/React). Canonical sync
encoding is produced by the Rust WASM bridge `scribe-web-wasm`
(`ignite/core/scribe-web-wasm`); TypeScript does not implement wire encoding.
Browser durable state uses IndexedDB `scribe-web-sync` (name preserved for
existing clients). PWA manifest `id` is the stable `/ignite-web` (not versioned).
Product mode pairs through `/control/v1` on `scribe-server`; developer-token
mode talks to `scribe-sync-server` over `/v1`.

## Ignite Server (Linux self-hosting)

Portable Linux packaging lives under `deployment/linux/` (bundle build, install /
upgrade, systemd unit, ownership-scoped firewall helpers, disposable staging
tests). On-disk paths remain `/etc/scribe-server` and `/var/lib/scribe-server`
for compatibility. See [`self-hosting.md`](self-hosting.md).

## Packaging and installation

1. Cross-compile ARMv7hf binaries (`tools/docker_build_armv7hf.sh`).
2. Build the pinned Dropbear client (`tools/dropbear/docker_build.sh` in the
   same digest-pinned `rust:1-bullseye` image as the Rust ARM build;
   `PINNED.txt` enforced).
3. Stage payload + build self-extracting Scriptlet
   (`deployment/kindle/package_release.py` via `scripts/kindle/package-release.sh` →
   `Install Ignite vVERSION.sh`).
4. Customer copies the single Scriptlet into Kindle **Documents** and opens it.

Installer logic and tests: `deployment/kindle/`.

## Experimental recovery subsystem

Modules under `scribe-app` (`sync_recovery_*`) implement planning, fencing, and
a supervised Dropbear `-B` HTTP stream for *future* controlled recovery. As
published:

- Live non-loopback recovery POST is compile-time disabled
- Production child spawn for the stream remains refused
- A test-only `kindle_supervisor_harness` feature can exercise the supervisor
  against disposable fixtures; it is excluded from customer packaging

Physical Kindle verification of the protected Dropbear client and
`SupervisedRecoveryStream` does **not** enable customer recovery.
Review 79 (performance) remains open and separate.
