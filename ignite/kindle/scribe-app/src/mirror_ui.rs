//! Amazon mirror glue for the runtime: what runs at the structural safe point (crash
//! recovery, the stat-only scan, one rebuild step), the Home card's Copy to Ignite, and the
//! Tools block refresh. Pure mirror logic lives in [`crate::mirror`]; this file only wires it
//! to the runtime's `UiExec`, logs and presents. Child of `runtime` so it can use its private
//! items; behaviour is the runtime's, unchanged.

use std::path::Path;
use std::time::Instant;

use super::{present_ui, KindleCtrl, UiExec};
use crate::app_ui::UiState;
use crate::home::AppView;
use crate::library::LibraryRepository;
use crate::mirror::{MirrorClock, MirrorRun};
use crate::modal::Modal;
use crate::ui_gfx::paper_background;

/// Crash recovery, once per process at the first safe point with sync linked
/// (`mirror::recovery_due`). Never aborts the session.
pub(super) fn recover(library: &mut LibraryRepository, clock: &mut MirrorClock) {
    clock.recovery_done = true;
    if crate::mirror::MirrorLedger::path_for(library.root()).exists() {
        match crate::mirror::recover_interrupted(library, &crate::nbk_import::GlobalSync) {
            Ok(n) => println!("mirror_recovered count={n}"),
            Err(e) => eprintln!("mirror_recovery_failed detail={e}"),
        }
    }
}

/// The stat-only scan: classify Amazon's notebooks against the ledger and queue a run for
/// the actions found. Returns true when Tools was refreshed and needs a full redraw.
pub(super) fn scan(
    ui: &mut UiState,
    library: &LibraryRepository,
    mirror_run: &mut Option<MirrorRun>,
    clock: &mut MirrorClock,
    now: Instant,
    batches: u64,
) -> bool {
    clock.last_scan = Some(now);
    clock.last_batches = batches;
    clock.last_scan_unix_ms = Some(crate::mirror::unix_now_ms());
    match crate::mirror::MirrorLedger::open(library.root()) {
        Ok(mut ledger) => {
            match crate::mirror::scan(&crate::nbk_import::ImportSources::kindle(), &ledger) {
                Ok(r) => {
                    println!(
                        "mirror_scan actions={} current={} unsupported={} skipped={}",
                        r.actions.len(),
                        r.current,
                        r.unsupported.len(),
                        r.skipped
                    );
                    clock.last_skipped = r.skipped;
                    if !r.actions.is_empty() {
                        *mirror_run = Some(crate::mirror::MirrorRun {
                            queue: r.actions.into(),
                            done: 0,
                            failed: vec![],
                        });
                    }
                    if let Err(e) = ledger.note_scan(None) {
                        eprintln!("mirror_scan_note_failed detail={e}");
                    }
                }
                Err(e) => {
                    eprintln!("mirror_scan_failed detail={e}");
                    if let Err(w) = ledger.note_scan(Some(e)) {
                        eprintln!("mirror_scan_note_failed detail={w}");
                    }
                }
            }
        }
        Err(e) => eprintln!("mirror_scan_failed detail={e}"),
    }
    if ui.view == AppView::Tools {
        refresh_tools(ui, library.root(), mirror_run.as_ref(), clock);
        return true;
    }
    false
}

/// Refresh the Tools screen's mirror block from the ledger (no Kindle file is touched).
pub(super) fn refresh_tools(
    ui: &mut UiState,
    library_root: &Path,
    run: Option<&crate::mirror::MirrorRun>,
    clock: &crate::mirror::MirrorClock,
) {
    let live = crate::mirror::MirrorLive {
        enabled: ui.mirror_enabled,
        sync_linked: crate::sync_outbound::with_global(|_| ()).is_some(),
        queued: run.map_or(0, |r| r.queue.len()),
        skipped: clock.last_skipped,
        last_scan_unix_ms: clock.last_scan_unix_ms,
    };
    let now = crate::mirror::unix_now_ms();
    ui.tools.mirror = match crate::mirror::MirrorLedger::open(library_root) {
        Ok(ledger) => crate::mirror::summary(&ledger, live, now),
        Err(e) => {
            eprintln!("mirror_summary_failed detail={e}");
            crate::mirror::MirrorSummary {
                enabled: live.enabled,
                sync_linked: live.sync_linked,
                skipped: live.skipped,
                pending: live.queued,
                last_scan_error: Some(e),
                now_unix_ms: now,
                ..Default::default()
            }
        }
    };
}

/// Home card menu → COPY TO IGNITE: import an editable copy of the mirror's Kindle source
/// (the regular one-way import, so a second copy is refused as already imported). Runs from
/// the Home modal at a safe point, never on the pen path; large notebooks take seconds, as
/// "Import all" does.
pub(super) fn copy(
    x: &mut UiExec<'_>,
    ctrl: &mut KindleCtrl,
    notebook_id: &str,
) -> Result<(), String> {
    use crate::nbk_import::{discover, import_one, GlobalSync, ImportOutcome, ImportSources};
    use crate::nbk_import::{ImportTarget, Ledger};
    let marker = x
        .library
        .notebook_dir(notebook_id)
        .join(crate::mirror::MIRROR_MARKER);
    let origin = std::fs::read(&marker)
        .map_err(|e| format!("read {}: {e}", marker.display()))
        .and_then(|b| {
            serde_json::from_slice::<crate::mirror::MirrorOriginV1>(&b)
                .map_err(|e| format!("unreadable mirror marker {}: {e}", marker.display()))
        });
    let origin_id = match origin {
        Ok(m) => m.origin_id,
        Err(e) => {
            eprintln!("copy_mirror_failed notebook={notebook_id} detail={e}");
            x.ui.show_error("The notebook couldn't be copied.");
            return present_ui(x, ctrl, None);
        }
    };
    let source = match discover(&ImportSources::kindle()) {
        Ok(all) => all.into_iter().find(|s| s.origin_id == origin_id),
        Err(e) => {
            eprintln!("copy_mirror_failed notebook={notebook_id} origin={origin_id} detail={e}");
            x.ui.show_error("The notebook couldn't be copied.");
            return present_ui(x, ctrl, None);
        }
    };
    let Some(source) = source else {
        println!("copy_mirror_missing notebook={notebook_id} origin={origin_id}");
        x.ui.show_error("That Kindle notebook is no longer on this device.");
        return present_ui(x, ctrl, None);
    };
    let paper = paper_background(&x.geo);
    let t0 = Instant::now();
    let outcome = match Ledger::open(x.library.root()) {
        Ok(mut ledger) => {
            let mut target = ImportTarget {
                library: &mut *x.library,
                profile: &*x.notebook_profile,
                geo: &x.geo,
                paper: &paper,
                cancel: x.stop,
            };
            import_one(&mut target, &mut ledger, &source, &GlobalSync)
        }
        Err(e) => ImportOutcome::Failed(e),
    };
    let ms = t0.elapsed().as_millis();
    match outcome {
        ImportOutcome::Imported {
            notebook_id: copy,
            pages,
            ..
        } => {
            println!("copy_mirror_done mirror={notebook_id} origin={origin_id} notebook={copy} pages={pages} ms={ms}");
        }
        ImportOutcome::AlreadyImported => {
            println!("copy_mirror_skipped mirror={notebook_id} origin={origin_id} reason=already_imported");
            x.ui.show_modal(Modal::already_copied_notice());
        }
        ImportOutcome::Failed(e) | ImportOutcome::Halted(e) => {
            eprintln!(
                "copy_mirror_failed notebook={notebook_id} origin={origin_id} ms={ms} detail={e}"
            );
            x.ui.show_error("The notebook couldn't be copied.");
        }
    }
    present_ui(x, ctrl, None)
}

/// Apply the next queued Amazon mirror action. Runs only at the structural safe point and only
/// where [`crate::mirror::step_allowed`] admits it (Home/Tools), never with a page open.
pub(super) fn step(x: &mut UiExec<'_>, ctrl: &mut KindleCtrl) -> Result<(), String> {
    use crate::mirror::{apply_one, kindle_decode, MirrorLedger, MirrorOutcome, MirrorTarget};
    use crate::nbk_import::{GlobalSync, ImportTarget};
    let Some(run) = x.mirror_run.as_mut() else {
        return Ok(());
    };
    let Some(action) = run.queue.pop_front() else {
        *x.mirror_run = None;
        return Ok(());
    };
    let paper = paper_background(&x.geo);
    let t0 = Instant::now();
    let outcome = match MirrorLedger::open(x.library.root()) {
        Ok(mut ledger) => {
            let mut target = MirrorTarget {
                import: ImportTarget {
                    library: &mut *x.library,
                    profile: &*x.notebook_profile,
                    geo: &x.geo,
                    paper: &paper,
                    cancel: x.stop,
                },
                folders: &mut *x.folders,
            };
            apply_one(
                &mut target,
                &mut ledger,
                &action,
                &kindle_decode,
                &GlobalSync,
            )
        }
        // An unreadable ledger fails closed for every action: stop the run.
        Err(e) => MirrorOutcome::Halted(e),
    };
    let ms = t0.elapsed().as_millis();
    let (label, detail) = match &outcome {
        MirrorOutcome::Done => ("done", ""),
        MirrorOutcome::Skipped(m) => ("skipped", m.as_str()),
        MirrorOutcome::Failed(m) => ("failed", m.as_str()),
        MirrorOutcome::Halted(m) => ("halted", m.as_str()),
    };
    println!(
        "mirror_step action={} origin={} outcome={label} ms={ms} detail={detail}",
        action.kind(),
        action.origin_id()
    );
    let finished = match outcome {
        MirrorOutcome::Done => {
            run.done += 1;
            run.queue.is_empty()
        }
        MirrorOutcome::Skipped(_) => run.queue.is_empty(),
        MirrorOutcome::Failed(m) => {
            run.failed.push((action.origin_id().to_string(), m));
            run.queue.is_empty()
        }
        MirrorOutcome::Halted(_) => {
            println!(
                "mirror_run_halted done={} failed={} not_started={}",
                run.done,
                run.failed.len(),
                run.queue.len()
            );
            *x.mirror_run = None;
            if x.ui.view == AppView::Tools && !x.ui.modal_open() {
                refresh_tools(x.ui, x.library.root(), None, x.mirror_clock);
                return present_ui(x, ctrl, None);
            }
            return Ok(());
        }
    };
    if finished {
        let done = run.done;
        println!("mirror_run_done done={done} failed={}", run.failed.len());
        *x.mirror_run = None;
        // Home lists notebooks and folders: show the mirrored result once, not per action.
        if done > 0 && x.ui.view == AppView::Home && !x.ui.modal_open() {
            return present_ui(x, ctrl, None);
        }
    }
    // Tools shows the mirror's progress: refresh it after every step.
    if x.ui.view == AppView::Tools && !x.ui.modal_open() {
        refresh_tools(
            x.ui,
            x.library.root(),
            x.mirror_run.as_ref(),
            x.mirror_clock,
        );
        return present_ui(x, ctrl, None);
    }
    Ok(())
}
