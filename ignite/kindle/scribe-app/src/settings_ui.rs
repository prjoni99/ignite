//! Settings chrome: peer tabs `TOOLS` and `PREFERENCES` (Home → tools icon).
//!
//! Tools still owns Import Kindle notebooks. Preferences owns replica-local options
//! (side-button action, Amazon mirror switch, open at launch). Neither view is a writing surface.

use kindle_platform::FbGeometry;

use crate::prefs::{OpenAtLaunch, SideButtonAction};
use crate::ui_gfx::{fit_text, text_width, wrap_text, Canvas, Rect, BLACK, GLYPH_H, GRAY, WHITE};

const MARGIN: u32 = 60;
const BACK_W: u32 = 330;
const BAR_H: u32 = 130;
const TOP: u32 = 40;
const TAB_GAP: u32 = 20;
pub const DIVIDER_Y: u32 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Tools,
    Preferences,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsChromeHit {
    Back,
    ToolsTab,
    PreferencesTab,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsChrome {
    pub back: Rect,
    pub tools_tab: Rect,
    pub prefs_tab: Rect,
    xres: u32,
    yres: u32,
}

impl SettingsChrome {
    pub fn new(geo: &FbGeometry) -> Self {
        let w = geo.xres.saturating_sub(2 * MARGIN);
        let back = Rect::new(MARGIN, TOP, BACK_W, BAR_H);
        let tabs_x = back.right() + TAB_GAP;
        let tabs_w = w.saturating_sub(BACK_W + TAB_GAP);
        let tab_w = tabs_w.saturating_sub(TAB_GAP) / 2;
        Self {
            back,
            tools_tab: Rect::new(tabs_x, TOP, tab_w, BAR_H),
            prefs_tab: Rect::new(tabs_x + tab_w + TAB_GAP, TOP, tab_w, BAR_H),
            xres: geo.xres,
            yres: geo.yres,
        }
    }

    pub fn hit(&self, x: u32, y: u32, back_enabled: bool) -> Option<SettingsChromeHit> {
        if self.back.contains(x, y) {
            return back_enabled.then_some(SettingsChromeHit::Back);
        }
        if self.tools_tab.contains(x, y) {
            return Some(SettingsChromeHit::ToolsTab);
        }
        if self.prefs_tab.contains(x, y) {
            return Some(SettingsChromeHit::PreferencesTab);
        }
        None
    }
}

/// Product version shown on Settings chrome. Sourced from the repo-root `VERSION` file.
pub fn product_version_label() -> String {
    format!("v{}", include_str!("../../../VERSION").trim())
}

const VERSION_SCALE: u32 = 5;

pub fn paint_settings_chrome(
    c: &mut Canvas<'_>,
    chrome: &SettingsChrome,
    selected: SettingsTab,
    back_enabled: bool,
) {
    let back_ink = if back_enabled { BLACK } else { GRAY };
    c.frame(chrome.back, 5, back_ink);
    c.text_centered(chrome.back.inset(8), 8, "< BACK", back_ink);
    paint_tab(c, chrome.tools_tab, "TOOLS", selected == SettingsTab::Tools);
    paint_tab(
        c,
        chrome.prefs_tab,
        "PREFERENCES",
        selected == SettingsTab::Preferences,
    );
    let label = product_version_label();
    let w = text_width(&label, VERSION_SCALE);
    let h = GLYPH_H * VERSION_SCALE;
    let x = chrome.xres.saturating_sub(MARGIN).saturating_sub(w);
    let y = chrome.yres.saturating_sub(MARGIN).saturating_sub(h);
    c.text(x, y, VERSION_SCALE, &label, GRAY);
}

fn paint_tab(c: &mut Canvas<'_>, r: Rect, label: &str, selected: bool) {
    if selected {
        c.fill(r, BLACK);
        c.text_centered(r.inset(8), 7, label, WHITE);
    } else {
        c.frame(r, 5, BLACK);
        c.text_centered(r.inset(8), 7, label, BLACK);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferencesHit {
    Chrome(SettingsChromeHit),
    NavigationUndo,
    CreateLink,
    /// Amazon mirror on/off.
    MirrorToggle,
    /// Open at launch: Home.
    LaunchHome,
    /// Open at launch: today's daily page.
    LaunchToday,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreferencesLayout {
    pub chrome: SettingsChrome,
    pub nav_undo: Rect,
    pub create_link: Rect,
    pub mirror_toggle: Rect,
    pub launch_home: Rect,
    pub launch_today: Rect,
}

const SECTION_Y: u32 = 260;
const BODY_Y: u32 = 360;
const CHOICE_Y: u32 = 720;
const CHOICE_H: u32 = 200;
const CHOICE_GAP: u32 = 40;
/// AMAZON MIRROR section below the side-button choices (heading, blurb, toggle row).
const MIRROR_SECTION_Y: u32 = CHOICE_Y + 2 * (CHOICE_H + CHOICE_GAP) + 80;
const MIRROR_BODY_Y: u32 = MIRROR_SECTION_Y + 100;
const MIRROR_ROW_Y: u32 = MIRROR_BODY_Y + 160;
/// OPEN AT LAUNCH section below the mirror row (heading, blurb, [Home] [Today] side by side).
const LAUNCH_SECTION_Y: u32 = MIRROR_ROW_Y + CHOICE_H + 80;
const LAUNCH_BODY_Y: u32 = LAUNCH_SECTION_Y + 100;
const LAUNCH_ROW_Y: u32 = LAUNCH_BODY_Y + 160;

impl PreferencesLayout {
    pub fn new(geo: &FbGeometry) -> Self {
        let w = geo.xres.saturating_sub(2 * MARGIN);
        let half = w.saturating_sub(CHOICE_GAP) / 2;
        Self {
            chrome: SettingsChrome::new(geo),
            nav_undo: Rect::new(MARGIN, CHOICE_Y, w, CHOICE_H),
            create_link: Rect::new(MARGIN, CHOICE_Y + CHOICE_H + CHOICE_GAP, w, CHOICE_H),
            mirror_toggle: Rect::new(MARGIN, MIRROR_ROW_Y, w, CHOICE_H),
            launch_home: Rect::new(MARGIN, LAUNCH_ROW_Y, half, CHOICE_H),
            launch_today: Rect::new(
                MARGIN + half + CHOICE_GAP,
                LAUNCH_ROW_Y,
                w.saturating_sub(half + CHOICE_GAP),
                CHOICE_H,
            ),
        }
    }

    pub fn hit(&self, x: u32, y: u32) -> Option<PreferencesHit> {
        if let Some(h) = self.chrome.hit(x, y, true) {
            return Some(PreferencesHit::Chrome(h));
        }
        if self.nav_undo.contains(x, y) {
            return Some(PreferencesHit::NavigationUndo);
        }
        if self.create_link.contains(x, y) {
            return Some(PreferencesHit::CreateLink);
        }
        if self.mirror_toggle.contains(x, y) {
            return Some(PreferencesHit::MirrorToggle);
        }
        if self.launch_home.contains(x, y) {
            return Some(PreferencesHit::LaunchHome);
        }
        if self.launch_today.contains(x, y) {
            return Some(PreferencesHit::LaunchToday);
        }
        None
    }
}

const SIDE_BUTTON_BLURB: &str = "Choose what the pen side button does while you are on a page.";
const MIRROR_BLURB: &str = "Keep a read-only copy of your Kindle notebooks in the Amazon folder.";
const LAUNCH_BLURB: &str = "Where Ignite goes when it starts.";

pub fn mirror_toggle_label(enabled: bool) -> &'static str {
    if enabled {
        "Mirror Amazon notebooks: On"
    } else {
        "Mirror Amazon notebooks: Off"
    }
}

pub fn render_preferences(
    fb: &mut [u8],
    geo: &FbGeometry,
    action: SideButtonAction,
    mirror_enabled: bool,
    open_at_launch: OpenAtLaunch,
) -> Result<(), String> {
    let layout = PreferencesLayout::new(geo);
    let mut c = Canvas::new(fb, geo)?;
    c.fill_all(WHITE);
    let w = geo.xres.saturating_sub(2 * MARGIN);
    paint_settings_chrome(&mut c, &layout.chrome, SettingsTab::Preferences, true);
    c.fill(Rect::new(MARGIN, DIVIDER_Y, w, 4), BLACK);
    c.text(MARGIN, SECTION_Y, 9, "SIDE BUTTON", BLACK);
    for (i, line) in wrap_text(SIDE_BUTTON_BLURB, 6, w, 5).iter().enumerate() {
        c.text(MARGIN, BODY_Y + i as u32 * 64, 6, line, BLACK);
    }
    paint_choice(
        &mut c,
        layout.nav_undo,
        "NAVIGATION + UNDO",
        "Page edges turn pages. Center undoes the last stroke.",
        action == SideButtonAction::NavigationUndo,
    );
    paint_choice(
        &mut c,
        layout.create_link,
        "CREATE LINK",
        "A side-button tap starts a link at that spot.",
        action == SideButtonAction::CreateLink,
    );
    c.text(MARGIN, MIRROR_SECTION_Y, 9, "AMAZON MIRROR", BLACK);
    for (i, line) in wrap_text(MIRROR_BLURB, 6, w, 2).iter().enumerate() {
        c.text(MARGIN, MIRROR_BODY_Y + i as u32 * 64, 6, line, BLACK);
    }
    let r = layout.mirror_toggle;
    c.frame(r, 5, BLACK);
    c.text(
        r.x + 40,
        r.y + 40,
        8,
        &fit_text(
            mirror_toggle_label(mirror_enabled),
            8,
            r.w.saturating_sub(80),
        ),
        BLACK,
    );
    c.text(
        r.x + 40,
        r.y + 120,
        5,
        if mirror_enabled {
            "Tap to turn off."
        } else {
            "Tap to turn on."
        },
        BLACK,
    );
    c.text(MARGIN, LAUNCH_SECTION_Y, 9, "OPEN AT LAUNCH", BLACK);
    for (i, line) in wrap_text(LAUNCH_BLURB, 6, w, 1).iter().enumerate() {
        c.text(MARGIN, LAUNCH_BODY_Y + i as u32 * 64, 6, line, BLACK);
    }
    paint_choice(
        &mut c,
        layout.launch_home,
        "Home",
        "Your notebooks.",
        open_at_launch == OpenAtLaunch::Home,
    );
    paint_choice(
        &mut c,
        layout.launch_today,
        "Today",
        "Today's daily page.",
        open_at_launch == OpenAtLaunch::Today,
    );
    Ok(())
}

fn paint_choice(c: &mut Canvas<'_>, r: Rect, title: &str, detail: &str, selected: bool) {
    if selected {
        c.fill(r, BLACK);
        c.text(r.x + 40, r.y + 40, 8, title, WHITE);
        c.text(
            r.x + 40,
            r.y + 120,
            5,
            &fit_text(detail, 5, r.w.saturating_sub(80)),
            WHITE,
        );
    } else {
        c.frame(r, 5, BLACK);
        c.text(r.x + 40, r.y + 40, 8, title, BLACK);
        c.text(
            r.x + 40,
            r.y + 120,
            5,
            &fit_text(detail, 5, r.w.saturating_sub(80)),
            BLACK,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_gfx::panel_geo;

    fn center(r: Rect) -> (u32, u32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    #[test]
    fn tabs_are_peers_and_choices_hit() {
        let g = panel_geo();
        let l = PreferencesLayout::new(&g);
        let (x, y) = center(l.chrome.tools_tab);
        assert_eq!(
            l.hit(x, y),
            Some(PreferencesHit::Chrome(SettingsChromeHit::ToolsTab))
        );
        let (x, y) = center(l.nav_undo);
        assert_eq!(l.hit(x, y), Some(PreferencesHit::NavigationUndo));
        let (x, y) = center(l.create_link);
        assert_eq!(l.hit(x, y), Some(PreferencesHit::CreateLink));
        let (x, y) = center(l.mirror_toggle);
        assert_eq!(l.hit(x, y), Some(PreferencesHit::MirrorToggle));
        // The toggle row sits below the side-button choices, same width, never overlapping.
        assert!(l.mirror_toggle.y >= l.create_link.bottom() + CHOICE_GAP);
        assert_eq!(
            (l.mirror_toggle.x, l.mirror_toggle.w, l.mirror_toggle.h),
            (l.create_link.x, l.create_link.w, CHOICE_H)
        );
        assert!(l.mirror_toggle.bottom() < g.yres - 200);
        assert_eq!(l.hit(x, l.create_link.bottom() + 1), None);
        assert_eq!(mirror_toggle_label(true), "Mirror Amazon notebooks: On");
        assert_eq!(mirror_toggle_label(false), "Mirror Amazon notebooks: Off");
        let mut fb = vec![0x55; g.smem_len as usize];
        for mirror in [false, true] {
            let home = OpenAtLaunch::Home;
            render_preferences(&mut fb, &g, SideButtonAction::NavigationUndo, mirror, home)
                .unwrap();
            render_preferences(&mut fb, &g, SideButtonAction::CreateLink, mirror, home).unwrap();
            for y in 0..g.yres as usize {
                let pad = &fb[y * g.line_length as usize + g.xres as usize
                    ..(y + 1) * g.line_length as usize];
                assert!(pad.iter().all(|b| *b == 0x55));
            }
        }
    }

    #[test]
    fn open_at_launch_rows_fit_below_the_mirror_and_hit() {
        let g = panel_geo();
        let l = PreferencesLayout::new(&g);
        assert!(l.launch_home.y >= l.mirror_toggle.bottom() + CHOICE_GAP);
        // Side by side: [Home] [Today], spanning the mirror row's width.
        assert_eq!(l.launch_today.y, l.launch_home.y);
        assert_eq!(l.launch_today.x, l.launch_home.right() + CHOICE_GAP);
        assert_eq!(l.launch_home.x, l.mirror_toggle.x);
        assert_eq!(l.launch_today.right(), l.mirror_toggle.right());
        assert_eq!((l.launch_home.h, l.launch_today.h), (CHOICE_H, CHOICE_H));
        // The last row ends above the bottom margin and the version label.
        let version_top = g.yres - MARGIN - GLYPH_H * VERSION_SCALE;
        assert!(
            l.launch_today.bottom() < g.yres - 60,
            "{:?}",
            l.launch_today
        );
        assert!(
            l.launch_today.bottom() < version_top,
            "{:?}",
            l.launch_today
        );
        let (x, y) = center(l.launch_home);
        assert_eq!(l.hit(x, y), Some(PreferencesHit::LaunchHome));
        let (x, y) = center(l.launch_today);
        assert_eq!(l.hit(x, y), Some(PreferencesHit::LaunchToday));
        assert_eq!(l.hit(l.launch_home.right() + 1, y), None);
        assert_eq!(l.hit(x, l.launch_today.bottom() + 1), None);
        let mut fb = vec![0x55; g.smem_len as usize];
        for launch in [OpenAtLaunch::Home, OpenAtLaunch::Today] {
            render_preferences(&mut fb, &g, SideButtonAction::NavigationUndo, true, launch)
                .unwrap();
            // The selected row is filled black.
            let r = match launch {
                OpenAtLaunch::Home => l.launch_home,
                OpenAtLaunch::Today => l.launch_today,
            };
            let px = fb[((r.y + 10) * g.line_length + r.x + 10) as usize];
            assert_eq!(px, BLACK);
        }
    }

    #[test]
    fn version_label_matches_root_version_file() {
        let raw = include_str!("../../../VERSION").trim();
        assert_eq!(product_version_label(), format!("v{raw}"));
        assert!(!raw.is_empty());
        assert!(raw.chars().all(|c| c.is_ascii_digit() || c == '.'));
    }

    #[test]
    fn version_text_is_not_a_hit_target() {
        let g = panel_geo();
        let chrome = SettingsChrome::new(&g);
        assert_eq!(chrome.hit(g.xres - 10, g.yres - 10, true), None);
        assert_eq!(
            chrome.hit(
                g.xres.saturating_sub(MARGIN),
                g.yres.saturating_sub(MARGIN),
                true
            ),
            None
        );
    }
}
