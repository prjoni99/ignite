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
  is created or removed.
- Cadence: a stat-only scan runs at launch, after each completed sync upload
  batch and every 15 minutes. A rebuild (decode + ordinary sync operations)
  runs one notebook per loop iteration. Both run only at the safe point: Home
  or Tools showing, pen idle, no page open. Never while sync is off or unlinked.
- Rebuild order: new pages and strokes first, old pages deleted after, so a
  notebook never drops below one page. Strokes flush to the journal in batches
  (same backpressure as "Import Kindle notebooks"). A crash mid-rebuild is
  cleaned up at next launch (partial mirror removed via the synced delete path,
  recreated by the next scan); never more than one mirror per source.
- Read-only: on the Kindle, pen/eraser, undo, page add/move/delete, link
  add/remove, lasso moves and rename are refused with a hint pointing to Copy
  to Ignite. In the browser companion, a notebook in a folder titled exactly
  `Amazon` shows a read-only banner and offers no Write/Type tools; moving it
  out lifts the convention (the next rebuild files it back).
- Copy to Ignite: the Home card menu runs the existing one-way import to make
  an editable copy; a source is copied at most once.
- Setting: Preferences → `AMAZON MIRROR` (`mirror_amazon` in replica-local
  `preferences.json`, default on). Tools shows status and `RESCAN NOW`.

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
