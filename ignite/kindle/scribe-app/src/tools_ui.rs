//! Tools screen (Home → tools icon). First tool: **Import Kindle notebooks** — a one-way copy of
//! Amazon notebooks into Scribe (`nbk_import`). Framebuffer-native like Sync: its own view, never
//! a writing surface, one GC16 FULL per change.

use kindle_platform::FbGeometry;

use crate::mirror::MirrorSummary;
use crate::settings_ui::{
    paint_settings_chrome, SettingsChrome, SettingsChromeHit, SettingsTab, DIVIDER_Y,
};
use crate::ui_gfx::{fit_text, wrap_text, Canvas, Rect, BLACK, WHITE};

const MARGIN: u32 = 60;
const SECTION_Y: u32 = 260;
const BODY_Y: u32 = 400;
const STATUS_Y: u32 = 720;
const STATUS_BOTTOM: u32 = 1350;
const STATUS_LINE_Y: u32 = 130;
const STATUS_LINE_STEP: u32 = 56;
const BUTTON_Y: u32 = 1390;
const BUTTON_H: u32 = 150;
const SECONDARY_Y: u32 = 1560;
const SECONDARY_H: u32 = 120;
/// Amazon mirror block: heading, status lines, then `RESCAN NOW` (above the version label).
const MIRROR_SECTION_Y: u32 = 1730;
const MIRROR_BODY_Y: u32 = 1810;
const MIRROR_LINE_STEP: u32 = 50;
/// Small enough that the 62-character summary line fits the panel width.
const MIRROR_SCALE: u32 = 4;
const RESCAN_Y: u32 = 2240;
const RESCAN_H: u32 = 120;
const MAX_FAILURE_LINES: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFailureLine {
    pub title: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolsPhase {
    /// Nothing scanned yet.
    Intro,
    Scanned {
        total: usize,
        eligible: usize,
        already_imported: usize,
    },
    Importing {
        done: usize,
        total: usize,
        current: String,
    },
    Finished {
        imported: usize,
        skipped: usize,
        failed: usize,
        failures: Vec<ImportFailureLine>,
    },
    /// Scan could not run (message for the user; details are logged).
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolsState {
    pub phase: ToolsPhase,
    /// Amazon mirror status, refreshed by the runtime on entry and after each mirror step.
    pub mirror: MirrorSummary,
}

impl Default for ToolsState {
    fn default() -> Self {
        Self {
            phase: ToolsPhase::Intro,
            mirror: MirrorSummary::default(),
        }
    }
}

impl ToolsState {
    pub fn busy(&self) -> bool {
        matches!(self.phase, ToolsPhase::Importing { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolsHit {
    Chrome(SettingsChromeHit),
    Scan,
    ImportAll,
    /// Amazon mirror: scan now instead of waiting for the next trigger.
    MirrorRescan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolsLayout {
    pub chrome: SettingsChrome,
    /// `(rect, label, action)`; absent while importing.
    pub primary: Option<(Rect, String, ToolsHit)>,
    pub secondary: Option<(Rect, String, ToolsHit)>,
    /// `RESCAN NOW`; only while the mirror is on and no import runs.
    pub rescan: Option<(Rect, String, ToolsHit)>,
}

impl ToolsLayout {
    pub fn new(geo: &FbGeometry, state: &ToolsState) -> Self {
        let w = geo.xres.saturating_sub(2 * MARGIN);
        let primary_rect = Rect::new(MARGIN, BUTTON_Y, w, BUTTON_H);
        let secondary_rect = Rect::new(MARGIN, SECONDARY_Y, w, SECONDARY_H);
        let scan_again = || (secondary_rect, "SCAN AGAIN".to_string(), ToolsHit::Scan);
        let (primary, secondary) = match &state.phase {
            ToolsPhase::Intro => (Some((primary_rect, "SCAN".into(), ToolsHit::Scan)), None),
            ToolsPhase::Scanned { eligible, .. } if *eligible > 0 => (
                Some((
                    primary_rect,
                    if *eligible == 1 {
                        "IMPORT 1 NOTEBOOK".into()
                    } else {
                        format!("IMPORT {eligible} NOTEBOOKS")
                    },
                    ToolsHit::ImportAll,
                )),
                Some(scan_again()),
            ),
            ToolsPhase::Importing { .. } => (None, None),
            ToolsPhase::Scanned { .. } | ToolsPhase::Finished { .. } | ToolsPhase::Error(_) => (
                Some((primary_rect, "SCAN AGAIN".into(), ToolsHit::Scan)),
                None,
            ),
        };
        let rescan = (state.mirror.enabled && !state.busy()).then(|| {
            (
                Rect::new(MARGIN, RESCAN_Y, w, RESCAN_H),
                "RESCAN NOW".to_string(),
                ToolsHit::MirrorRescan,
            )
        });
        Self {
            chrome: SettingsChrome::new(geo),
            primary,
            secondary,
            rescan,
        }
    }

    /// Taps resolve against what is drawn; Back is inert while an import runs.
    pub fn hit(&self, x: u32, y: u32, state: &ToolsState) -> Option<ToolsHit> {
        if let Some(h) = self.chrome.hit(x, y, !state.busy()) {
            return Some(ToolsHit::Chrome(h));
        }
        [&self.primary, &self.secondary, &self.rescan]
            .into_iter()
            .flatten()
            .find(|(r, _, _)| r.contains(x, y))
            .map(|(_, _, h)| *h)
    }
}

pub const EXPLANATION: &str = "Copies your existing Kindle notebooks into Ignite. \
    The original Kindle notebooks are never changed. Each Kindle notebook is imported only once.";

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Headline + detail lines for the current phase, then the Amazon mirror block.
pub fn status_lines(state: &ToolsState) -> (String, Vec<String>) {
    let (headline, mut lines) = import_lines(state);
    lines.extend(mirror_lines(&state.mirror));
    (headline, lines)
}

/// `Xs ago` under a minute, `X min ago` under an hour, else `X h ago`.
fn ago(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else {
        format!("{} h ago", s / 3600)
    }
}

/// The Amazon mirror block: one summary line, the last scan error, the last scan's skipped
/// sources, then unsupported notebooks.
pub fn mirror_lines(m: &MirrorSummary) -> Vec<String> {
    let last = match m.last_scan_unix_ms {
        Some(t) => ago(m.now_unix_ms.saturating_sub(t)),
        None => "never".into(),
    };
    let state = match (m.enabled, m.sync_linked) {
        (false, _) => "off",
        (true, true) => "on",
        (true, false) => "on (waiting for sync)",
    };
    let mut lines = vec![format!(
        "Amazon mirror: {state}, {} current, {} pending, last scan {last}",
        m.current, m.pending
    )];
    if let Some(e) = &m.last_scan_error {
        lines.push(format!("last scan failed: {e}"));
    }
    if m.skipped > 0 {
        lines.push(format!(
            "last scan skipped {} listed but missing on disk",
            m.skipped
        ));
    }
    for (title, error) in m.unsupported.iter().take(MAX_FAILURE_LINES) {
        lines.push(format!("{title}: {}", failure_reason(error)));
    }
    if m.unsupported.len() > MAX_FAILURE_LINES {
        lines.push(format!(
            "...and {} more",
            m.unsupported.len() - MAX_FAILURE_LINES
        ));
    }
    lines
}

/// Headline + detail lines for the import phase alone.
fn import_lines(state: &ToolsState) -> (String, Vec<String>) {
    match &state.phase {
        ToolsPhase::Intro => (
            "Scan to find Kindle notebooks.".into(),
            vec!["Nothing is copied until you choose Import.".into()],
        ),
        ToolsPhase::Scanned {
            total,
            eligible,
            already_imported,
        } => (
            format!(
                "Found {}.",
                plural(*total, "Kindle notebook", "Kindle notebooks")
            ),
            vec![
                format!("{eligible} ready to import"),
                format!("{already_imported} already imported"),
            ],
        ),
        ToolsPhase::Importing {
            done,
            total,
            current,
        } => (
            format!("Importing {} of {total}...", done + 1),
            vec![current.clone(), "Please wait.".into()],
        ),
        ToolsPhase::Finished {
            imported,
            skipped,
            failed,
            failures,
        } => {
            let mut lines = vec![
                format!("Imported: {imported}"),
                format!("Skipped (already imported): {skipped}"),
                format!("Failed: {failed}"),
            ];
            for f in failures.iter().take(MAX_FAILURE_LINES) {
                lines.push(format!("{}: {}", f.title, f.reason));
            }
            if failures.len() > MAX_FAILURE_LINES {
                lines.push(format!(
                    "...and {} more",
                    failures.len() - MAX_FAILURE_LINES
                ));
            }
            ("Import finished.".into(), lines)
        }
        ToolsPhase::Error(msg) => ("Scan failed.".into(), vec![msg.clone()]),
    }
}

/// Short user-facing reason for an import failure code (details stay in the log).
pub fn failure_reason(detail: &str) -> String {
    if detail.starts_with("sync halted") {
        return "sync stopped; import paused".into();
    }
    if detail.starts_with("import cancelled") {
        return "interrupted; try again".into();
    }
    let code = detail.split(':').next().unwrap_or_default();
    match code {
        "NBK_PENDING_ACTIONS" => "open it on the Kindle, then try again".into(),
        "SOURCE_BUSY" | "SOURCE_CHANGED_DURING_READ" => "Kindle was saving it; try again".into(),
        "NBK_CONTAINER" | "NBK_UNSUPPORTED" => "format not supported".into(),
        _ => "could not be imported".into(),
    }
}

pub fn render_tools(fb: &mut [u8], geo: &FbGeometry, state: &ToolsState) -> Result<(), String> {
    let layout = ToolsLayout::new(geo, state);
    let mut c = Canvas::new(fb, geo)?;
    c.fill_all(WHITE);
    let w = geo.xres.saturating_sub(2 * MARGIN);

    paint_settings_chrome(&mut c, &layout.chrome, SettingsTab::Tools, !state.busy());
    c.fill(Rect::new(MARGIN, DIVIDER_Y, w, 4), BLACK);

    c.text(MARGIN, SECTION_Y, 9, "IMPORT KINDLE NOTEBOOKS", BLACK);
    for (i, line) in wrap_text(EXPLANATION, 6, w, 5).iter().enumerate() {
        c.text(MARGIN, BODY_Y + i as u32 * 64, 6, line, BLACK);
    }

    let status = Rect::new(MARGIN, STATUS_Y, w, STATUS_BOTTOM - STATUS_Y);
    c.frame(status, 4, BLACK);
    let (headline, details) = import_lines(state);
    c.text(
        status.x + 40,
        status.y + 40,
        8,
        &fit_text(&headline, 8, status.w - 80),
        BLACK,
    );
    for (i, line) in details.iter().enumerate() {
        let y = status.y + STATUS_LINE_Y + i as u32 * STATUS_LINE_STEP;
        if y + 50 > status.bottom() {
            break;
        }
        c.text(
            status.x + 40,
            y,
            6,
            &fit_text(line, 6, status.w - 80),
            BLACK,
        );
    }

    if let Some((r, label, _)) = &layout.primary {
        c.fill(*r, BLACK);
        c.text_centered(r.inset(10), 9, label, WHITE);
    }
    if let Some((r, label, _)) = &layout.secondary {
        c.frame(*r, 5, BLACK);
        c.text_centered(r.inset(10), 8, label, BLACK);
    }

    c.text(MARGIN, MIRROR_SECTION_Y, 7, "AMAZON MIRROR", BLACK);
    for (i, line) in mirror_lines(&state.mirror).iter().enumerate() {
        let y = MIRROR_BODY_Y + i as u32 * MIRROR_LINE_STEP;
        c.text(
            MARGIN,
            y,
            MIRROR_SCALE,
            &fit_text(line, MIRROR_SCALE, w),
            BLACK,
        );
    }
    if let Some((r, label, _)) = &layout.rescan {
        c.frame(*r, 5, BLACK);
        c.text_centered(r.inset(10), 7, label, BLACK);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_gfx::panel_geo;

    fn center(r: Rect) -> (u32, u32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    #[test]
    fn each_phase_offers_the_right_action_and_back_is_inert_while_importing() {
        let g = panel_geo();
        let intro = ToolsState::default();
        let l = ToolsLayout::new(&g, &intro);
        let (x, y) = center(l.primary.as_ref().unwrap().0);
        assert_eq!(l.hit(x, y, &intro), Some(ToolsHit::Scan));
        let (bx, by) = center(l.chrome.back);
        assert_eq!(
            l.hit(bx, by, &intro),
            Some(ToolsHit::Chrome(SettingsChromeHit::Back))
        );

        let scanned = ToolsState {
            phase: ToolsPhase::Scanned {
                total: 3,
                eligible: 2,
                already_imported: 1,
            },
            ..ToolsState::default()
        };
        let l = ToolsLayout::new(&g, &scanned);
        let (label, hit) = (
            &l.primary.as_ref().unwrap().1,
            l.primary.as_ref().unwrap().2,
        );
        assert_eq!(
            (label.as_str(), hit),
            ("IMPORT 2 NOTEBOOKS", ToolsHit::ImportAll)
        );
        let (x, y) = center(l.secondary.as_ref().unwrap().0);
        assert_eq!(l.hit(x, y, &scanned), Some(ToolsHit::Scan));

        let busy = ToolsState {
            phase: ToolsPhase::Importing {
                done: 0,
                total: 2,
                current: "Journal".into(),
            },
            ..ToolsState::default()
        };
        let l = ToolsLayout::new(&g, &busy);
        assert!(l.primary.is_none() && l.secondary.is_none());
        assert_eq!(l.hit(bx, by, &busy), None);
    }

    #[test]
    fn renders_every_phase_without_touching_stride_padding() {
        let g = panel_geo();
        for phase in [
            ToolsPhase::Intro,
            ToolsPhase::Scanned {
                total: 21,
                eligible: 21,
                already_imported: 0,
            },
            ToolsPhase::Importing {
                done: 3,
                total: 21,
                current: "Rumi Scratch Pad".into(),
            },
            ToolsPhase::Finished {
                imported: 19,
                skipped: 1,
                failed: 8,
                failures: (0..8)
                    .map(|i| ImportFailureLine {
                        title: format!("Notebook {i}"),
                        reason: failure_reason("NBK_PENDING_ACTIONS: x"),
                    })
                    .collect(),
            },
            ToolsPhase::Error("Kindle notebook catalog unreadable".into()),
        ] {
            let mut fb = vec![0x55; g.smem_len as usize];
            render_tools(
                &mut fb,
                &g,
                &ToolsState {
                    phase,
                    mirror: MirrorSummary::default(),
                },
            )
            .unwrap();
            for y in 0..g.yres as usize {
                let pad = &fb[y * g.line_length as usize + g.xres as usize
                    ..(y + 1) * g.line_length as usize];
                assert!(pad.iter().all(|b| *b == 0x55));
            }
        }
        assert_eq!(
            status_lines(&ToolsState {
                phase: ToolsPhase::Scanned {
                    total: 1,
                    eligible: 1,
                    already_imported: 0
                },
                ..ToolsState::default()
            })
            .0,
            "Found 1 Kindle notebook."
        );
    }

    fn mirror(enabled: bool, last_scan_age_ms: Option<u64>) -> MirrorSummary {
        let now = 10_000_000;
        MirrorSummary {
            enabled,
            sync_linked: true,
            skipped: 0,
            current: 12,
            pending: 1,
            last_scan_unix_ms: last_scan_age_ms.map(|a| now - a),
            last_scan_error: None,
            unsupported: Vec::new(),
            now_unix_ms: now,
        }
    }

    fn with_mirror(m: MirrorSummary) -> ToolsState {
        ToolsState {
            mirror: m,
            ..ToolsState::default()
        }
    }

    #[test]
    fn status_lines_append_the_mirror_block() {
        let (_, lines) = status_lines(&with_mirror(mirror(true, Some(200_000))));
        assert_eq!(
            lines,
            vec![
                "Nothing is copied until you choose Import.".to_string(),
                "Amazon mirror: on, 12 current, 1 pending, last scan 3 min ago".into(),
            ]
        );
        let (_, lines) = status_lines(&with_mirror(MirrorSummary {
            current: 0,
            pending: 0,
            ..mirror(false, None)
        }));
        assert_eq!(
            lines.last().unwrap(),
            "Amazon mirror: off, 0 current, 0 pending, last scan never"
        );
        // On but sync not linked (off, faulted or not yet paired): nothing runs until it is.
        let (_, lines) = status_lines(&with_mirror(MirrorSummary {
            sync_linked: false,
            ..mirror(true, None)
        }));
        assert_eq!(
            lines.last().unwrap(),
            "Amazon mirror: on (waiting for sync), 12 current, 1 pending, last scan never"
        );
        // Off wins over unlinked.
        let lines = mirror_lines(&MirrorSummary {
            sync_linked: false,
            ..mirror(false, None)
        });
        assert!(lines[0].starts_with("Amazon mirror: off,"), "{}", lines[0]);
        for (age, text) in [
            (0, "last scan 0s ago"),
            (59_999, "last scan 59s ago"),
            (60_000, "last scan 1 min ago"),
            (3_599_999, "last scan 59 min ago"),
            (3_600_000, "last scan 1 h ago"),
            (7_300_000, "last scan 2 h ago"),
        ] {
            let lines = mirror_lines(&mirror(true, Some(age)));
            assert!(lines[0].ends_with(text), "{age}: {}", lines[0]);
        }
    }

    #[test]
    fn mirror_block_shows_the_skipped_count_only_when_nonzero() {
        assert_eq!(mirror_lines(&mirror(true, Some(1_000))).len(), 1);
        let lines = mirror_lines(&MirrorSummary {
            skipped: 2,
            ..mirror(true, Some(1_000))
        });
        assert_eq!(
            lines,
            vec![
                "Amazon mirror: on, 12 current, 1 pending, last scan 1s ago".to_string(),
                "last scan skipped 2 listed but missing on disk".into(),
            ]
        );
    }

    #[test]
    fn mirror_block_shows_the_scan_error_and_at_most_five_unsupported() {
        let m = MirrorSummary {
            last_scan_error: Some("Kindle catalog: locked".into()),
            skipped: 1,
            unsupported: (0..6)
                .map(|i| (format!("Notebook {i}"), "NBK_UNSUPPORTED: v9".to_string()))
                .collect(),
            ..mirror(true, Some(1_000))
        };
        let (_, lines) = status_lines(&with_mirror(m.clone()));
        let block: Vec<&str> = lines.iter().skip(1).map(String::as_str).collect();
        assert_eq!(
            block,
            vec![
                "Amazon mirror: on, 12 current, 1 pending, last scan 1s ago",
                "last scan failed: Kindle catalog: locked",
                "last scan skipped 1 listed but missing on disk",
                "Notebook 0: format not supported",
                "Notebook 1: format not supported",
                "Notebook 2: format not supported",
                "Notebook 3: format not supported",
                "Notebook 4: format not supported",
                "...and 1 more",
            ]
        );
        // Every phase and the full mirror block still render inside the framebuffer.
        let g = panel_geo();
        let mut fb = vec![0x55; g.smem_len as usize];
        render_tools(&mut fb, &g, &with_mirror(m)).unwrap();
    }

    #[test]
    fn rescan_now_is_offered_only_while_the_mirror_is_on() {
        let g = panel_geo();
        let on = with_mirror(mirror(true, None));
        let l = ToolsLayout::new(&g, &on);
        let (r, label, hit) = l.rescan.as_ref().unwrap();
        assert_eq!(
            (label.as_str(), *hit),
            ("RESCAN NOW", ToolsHit::MirrorRescan)
        );
        let (x, y) = center(*r);
        assert_eq!(l.hit(x, y, &on), Some(ToolsHit::MirrorRescan));
        // The import buttons still resolve.
        let (x, y) = center(l.primary.as_ref().unwrap().0);
        assert_eq!(l.hit(x, y, &on), Some(ToolsHit::Scan));
        let off = with_mirror(mirror(false, None));
        assert!(ToolsLayout::new(&g, &off).rescan.is_none());
        let busy = ToolsState {
            phase: ToolsPhase::Importing {
                done: 0,
                total: 2,
                current: "Journal".into(),
            },
            mirror: mirror(true, None),
        };
        assert!(ToolsLayout::new(&g, &busy).rescan.is_none());
    }
}
