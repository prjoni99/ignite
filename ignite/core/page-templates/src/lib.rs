//! Parametric page backgrounds rendered as gray8 bitmaps (one byte per pixel,
//! row-major, `0xFF` white). Pure: no I/O, no randomness, no time.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PANEL_W: u32 = 1860;
pub const PANEL_H: u32 = 2480;
/// Rows reserved for the app's top chrome; always `WHITE`.
pub const SAFE_TOP: u32 = 130;
/// Rows reserved for the app's bottom chrome; always `WHITE`.
pub const SAFE_BOTTOM: u32 = 57;
pub const LINE: u8 = 0xC0;
pub const DOT: u8 = 0xA0;
pub const WHITE: u8 = 0xFF;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplateSpec {
    Blank,
    Lined { spacing_px: u32, margin_px: u32 },
    DotGrid { pitch_px: u32 },
    Graph { pitch_px: u32 },
    Cornell,
    Daily,
    Weekly,
    Isometric { pitch_px: u32 },
    MusicStaff,
}

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub spec: TemplateSpec,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin { id: "blank", name: "Blank", spec: TemplateSpec::Blank },
    Builtin { id: "lined", name: "Lined", spec: TemplateSpec::Lined { spacing_px: LINED_SPACING, margin_px: LINED_MARGIN } },
    Builtin { id: "dot_grid", name: "Dot grid", spec: TemplateSpec::DotGrid { pitch_px: 60 } },
    Builtin { id: "graph", name: "Graph", spec: TemplateSpec::Graph { pitch_px: 60 } },
    Builtin { id: "cornell", name: "Cornell", spec: TemplateSpec::Cornell },
    Builtin { id: "daily", name: "Daily", spec: TemplateSpec::Daily },
    Builtin { id: "weekly", name: "Weekly", spec: TemplateSpec::Weekly },
    Builtin { id: "isometric", name: "Isometric", spec: TemplateSpec::Isometric { pitch_px: 80 } },
    Builtin { id: "music_staff", name: "Music staff", spec: TemplateSpec::MusicStaff },
];

const LINED_SPACING: u32 = 94;
const LINED_MARGIN: u32 = 160;
const CORNELL_CUE_X: u32 = 560;
const CORNELL_SUMMARY_FROM_BOTTOM: u32 = 420;
const DAILY_HEADER: u32 = 300;
const WEEK_BANDS: u32 = 7;
const STAFF_LINE_GAP: u32 = 24;
const STAFF_GROUP_PITCH: u32 = 260;
const STAFF_MARGIN: u32 = 80;

struct Canvas {
    px: Vec<u8>,
    w: u32,
    h: u32,
}

impl Canvas {
    /// Fill the half-open rectangle `[x0, x1) x [y0, y1)`, clipped to the canvas.
    fn rect(&mut self, x0: u32, x1: u32, y0: u32, y1: u32, v: u8) {
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                self.px[(y * self.w + x) as usize] = v;
            }
        }
    }

    fn hline(&mut self, y: u32, x0: u32, x1: u32, thick: u32, v: u8) {
        self.rect(x0, x1, y, y.saturating_add(thick), v);
    }

    fn vline(&mut self, x: u32, y0: u32, y1: u32, thick: u32, v: u8) {
        self.rect(x, x.saturating_add(thick), y0, y1, v);
    }

    fn lined(&mut self, from_y: u32, spacing: u32, margin: u32) {
        if spacing == 0 {
            return;
        }
        let end = self.h.saturating_sub(SAFE_BOTTOM);
        let x1 = self.w.saturating_sub(margin);
        let mut y = from_y;
        while y.saturating_add(2) <= end {
            self.hline(y, margin, x1, 2, LINE);
            y = y.saturating_add(spacing);
        }
    }
}

pub fn render(spec: &TemplateSpec, w: u32, h: u32) -> Vec<u8> {
    let mut c = Canvas { px: vec![WHITE; w as usize * h as usize], w, h };
    let top = SAFE_TOP;
    let bottom = h.saturating_sub(SAFE_BOTTOM);
    match *spec {
        TemplateSpec::Blank => {}
        TemplateSpec::Lined { spacing_px, margin_px } => {
            c.lined(top.saturating_add(spacing_px), spacing_px, margin_px);
        }
        TemplateSpec::DotGrid { pitch_px } => {
            if pitch_px > 0 {
                let mut y = top.saturating_add(pitch_px);
                while y < bottom {
                    let mut x = pitch_px;
                    while x < w {
                        c.rect(x, x.saturating_add(2), y, y.saturating_add(2), DOT);
                        x = x.saturating_add(pitch_px);
                    }
                    y = y.saturating_add(pitch_px);
                }
            }
        }
        TemplateSpec::Graph { pitch_px } => {
            if pitch_px > 0 {
                let mut y = top;
                while y < bottom {
                    c.hline(y, 0, w, 1, LINE);
                    y = y.saturating_add(pitch_px);
                }
                let mut x = pitch_px;
                while x < w {
                    c.vline(x, top, bottom, 1, LINE);
                    x = x.saturating_add(pitch_px);
                }
            }
        }
        TemplateSpec::Cornell => {
            let summary = h.saturating_sub(SAFE_BOTTOM + CORNELL_SUMMARY_FROM_BOTTOM);
            c.vline(CORNELL_CUE_X, top, summary, 2, LINE);
            c.hline(summary, 0, w, 2, LINE);
        }
        TemplateSpec::Daily => {
            let rule = top + DAILY_HEADER;
            c.hline(rule, 0, w, 2, LINE);
            c.lined(rule + LINED_SPACING, LINED_SPACING, LINED_MARGIN);
        }
        TemplateSpec::Weekly => {
            let band = bottom.saturating_sub(top) / WEEK_BANDS;
            for i in 1..WEEK_BANDS {
                c.hline(top + i * band, 0, w, 2, LINE);
            }
        }
        TemplateSpec::Isometric { pitch_px } => {
            if pitch_px > 0 {
                // Triangular lattice: row height is ~0.866 * pitch, odd rows offset by half a pitch.
                let row_h = (pitch_px - pitch_px / 8).max(1);
                let mut y = top.saturating_add(row_h);
                let mut odd = false;
                while y < bottom {
                    let mut x = if odd { pitch_px / 2 } else { 0 };
                    while x < w {
                        c.rect(x, x.saturating_add(2), y, y.saturating_add(2), DOT);
                        x = x.saturating_add(pitch_px);
                    }
                    odd = !odd;
                    y = y.saturating_add(row_h);
                }
            }
        }
        TemplateSpec::MusicStaff => {
            let mut g = top + STAFF_GROUP_PITCH / 2;
            while g + 4 * STAFF_LINE_GAP < bottom {
                for i in 0..5 {
                    c.hline(g + i * STAFF_LINE_GAP, STAFF_MARGIN, w.saturating_sub(STAFF_MARGIN), 1, LINE);
                }
                g += STAFF_GROUP_PITCH;
            }
        }
    }
    // Chrome rows are always white so a copied page hashes equal after the chrome scrub.
    c.rect(0, w, 0, top, WHITE);
    c.rect(0, w, bottom, h, WHITE);
    c.px
}

/// Box-average `factor x factor` blocks (round to nearest); output `(w/factor) * (h/factor)`.
/// If `bitmap` is shorter than `w * h`, returns an all-white thumbnail of the expected size
/// instead of panicking.
pub fn thumbnail(bitmap: &[u8], w: u32, h: u32, factor: u32) -> Vec<u8> {
    if factor == 0 {
        return Vec::new();
    }
    let (tw, th) = (w / factor, h / factor);
    let n = factor as u64 * factor as u64;
    if (bitmap.len() as u64) < w as u64 * h as u64 {
        return vec![WHITE; tw as usize * th as usize];
    }
    let mut out = Vec::with_capacity(tw as usize * th as usize);
    for ty in 0..th {
        for tx in 0..tw {
            let mut sum = 0u64;
            for dy in 0..factor {
                let row = ((ty * factor + dy) as usize) * w as usize + (tx * factor) as usize;
                sum += bitmap[row..row + factor as usize].iter().map(|&v| v as u64).sum::<u64>();
            }
            out.push(((sum + n / 2) / n) as u8);
        }
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
