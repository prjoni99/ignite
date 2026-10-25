# Ignite troubleshooting (Kindle)

Verified behavior from installer scripts and Diagnose helpers. This is not a
guarantee for every firmware.

## Prerequisites

Ignite needs a Kindle **already prepared** for third-party apps:

- Jailbreak / community firmware tooling
- SH Integration (or equivalent Scriptlet runner)
- A working root helper (commonly `su` / `sudo` on the device)

Ignite **will not** jailbreak a stock Kindle. If the installer reports a missing
privilege helper, stop and prepare the device first using the community guide at
<https://kindlemodding.org/jailbreaking/>.

## Missing privilege helper

**Symptom:** Install fails mentioning root helper / `su` / preparation.

**Meaning:** The device is not prepared for third-party privileged install.

**Action:** Complete jailbreak / SH Integration setup via
<https://kindlemodding.org/jailbreaking/>; do not expect Ignite to
bypass this.

## SH Integration / Scriptlet problems

**Symptom:** `Install Ignite v*.sh` does not appear, will not open, or exits
immediately.

**Checks:**

1. File is in **Documents** (not a nested ZIP folder).
2. Filename starts with `Install Ignite` as packaged.
3. SH Integration / Scriptlet support is installed for your firmware.
4. Copy the file again over USB; avoid editors that change line endings.

## App registration issues

**Symptom:** Install claims success but Ignite never appears in the Library, or
opens the wrong command.

**Action:**

1. Open **Diagnose Ignite** (or copy `Ignite-Diagnose.txt` from Documents).
2. Confirm `registration_state=canonical_only` and handler `com.markrai.ignite`
   points at `/mnt/us/scribe-app/bin/scribe-lifecycle-host.sh`.
3. If Diagnose reports `legacy_only` or `both_present`, re-run **Install Ignite**
   from Home (app not running) to finish the `com.markrai.scribe` →
   `com.markrai.ignite` migration.
4. If needed, **Uninstall Ignite** (notebooks kept) then reinstall.

## Launch returns to Kindle Home

**Symptom:** Tapping Ignite briefly shows a splash or blank panel then returns
to Home / Library.

**Common causes:**

- Binary/architecture mismatch (need ARMv7hf build for the Scribe)
- Lifecycle host misconfigured (`--data-dir` / `--sync-dir` missing)
- App crashed on startup (panel / permissions)

**Action:** Collect Diagnose output; reinstall matching Scriptlet version; ensure
you did not mix host scripts from an older package with new binaries.

## Diagnostic Scriptlet

After a successful unpack, **Diagnose Ignite** is registered. It writes
`Ignite-Diagnose.txt` under Documents even when Library registration is
incomplete (installer also writes a report on failure paths).

Copy that file over USB when asking for help. Redact personal notebook titles if
you paste excerpts publicly.

## Version and firmware reporting

Diagnose includes package version and device/firmware fields. When reporting
issues, include:

- Ignite version (filename / Diagnose)
- Kindle generation (e.g. Scribe 1st gen / 2024 Scribe)
- Firmware string (e.g. 5.18.6)
- Panel polarity / grayscale notes if ink looks inverted

## Safe uninstall

**Uninstall Ignite** removes application files and Library registration.
**Notebooks under the data directory are preserved** by design.

Always run **Backup Ignite Data** before major experiments and copy the backup
folder off-device over USB.

## Sync / pairing (optional)

Ordinary handwriting does not require a server. If you enable self-hosted sync:

- Use a trusted LAN
- Do not expose sync ports to the public Internet without understanding the
  threat model
- Do not enable experimental server lineage-replacement acceptance unless you
  are following a controlled recovery procedure (not part of normal install)

## Amazon mirror (optional)

Stock Kindle notebooks are mirrored read-only into the `Amazon` folder when
sync is linked. See `docs/architecture.md`.

**Notebook not appearing under Amazon.** Check in order:

1. Sync is linked (Sync view); the mirror never runs while sync is off.
2. Preferences → `AMAZON MIRROR` → `Mirror Amazon notebooks` is On.
3. The Kindle has finished saving the notebook; a source still being written
   or whose action log is ahead of its saved state is not listed anywhere and
   is retried at the next scan.
4. Tools → MIRROR block: an unreadable or unsupported source (`NBK_CONTAINER`,
   `NBK_UNSUPPORTED`) is listed there and not retried until it changes.
5. Press `RESCAN NOW` in Tools.

**Nothing happens while writing.** Expected: the mirror does nothing with a page
open.

**Mirror paused, nothing removed.** If the notebooks root or catalog is
missing (storage not mounted), or a listed source's directory or `nbk` is
missing, the mirror takes no action for it rather than removing anything.

**Can't edit a mirror.** On the Kindle an open mirror refuses edits with a
`Read-only mirror` notice, and its card menu offers only `COPY TO IGNITE` and
`DELETE`. Use Copy to Ignite for an editable notebook. Renaming the `Amazon`
folder or moving a notebook out of it (browser or Kindle) lifts the browser's
read-only treatment, but any ink or text added afterwards is discarded at the
next rebuild.

**Removed from Amazon.** The source was deleted on the Kindle; its mirror is
kept there, never deleted. It returns to `Amazon` if the source reappears. A
mirror you deleted in Ignite is recreated at the source's next change.

**Log lines.** `mirror_scan`, `mirror_step` and `mirror_recovered` appear in
the app log.

## Templates, Daily page and Capture

See `docs/architecture.md` for how these work.

**Custom PNG not listed, or listed as unreadable.** Copy it over USB to
`<library root>/templates/custom/` (default `/mnt/us/scribe-data/templates/custom/`).
It is listed by file name without extension. A file that is not a decodable
PNG is shown as unreadable and skipped; re-export it as PNG. Picking a
template that fails to load shows `That template couldn't be loaded.` and
creates nothing.

**`Clock not set` / `Set the Kindle clock first.`** `TODAY` needs the Kindle's
clock; one reading before 2025 creates nothing. Set the clock, then retry.
(`CAPTURE` still works with the clock unset, but skips the time stamp.)

**Where did `Daily` or `Inbox` go?** Both are tracked by id, so renaming or
moving them is fine; look for the renamed notebook or folder. A deleted month
notebook is recreated at the next `TODAY`, a deleted `Daily` folder at the next
month. A corrupt `<library root>/.scribe-daily/daily.json` shows an error and
creates nothing; `<library root>/.scribe-capture/capture.json` tracks the
Inbox the same way. `Couldn't open the Inbox.` means the capture failed.

**Side-button capture.** With `SIDE BUTTON` set to `Capture` in Preferences, pressing the pen's side button and touching the page jumps to a fresh Inbox page. The same gesture on that Inbox page returns to the notebook and page you came from, or to Home when the capture started there.

**An empty capture page vanished.** By design: leaving a capture page
discards it if nothing was written, it has no links or text beyond the stamp,
and it is not the Inbox's only page.

**Log lines.** `template_load_failed`, `daily_stamped`, `daily_open_failed`,
`daily_open_refused`, `capture_stamped`, `capture_stamp_skipped`,
`capture_discarded`, `capture_discard_failed`, `capture_failed`.
