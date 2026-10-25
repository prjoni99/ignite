use page_templates::*;

#[test]
fn every_builtin_renders_the_panel_size_and_keeps_the_safe_rows_white() {
    for b in BUILTINS {
        let px = render(&b.spec, PANEL_W, PANEL_H);
        assert_eq!(px.len(), (PANEL_W * PANEL_H) as usize);
        assert!(px[..(PANEL_W * SAFE_TOP) as usize].iter().all(|&v| v == WHITE), "{}", b.id);
        assert!(px[((PANEL_H - SAFE_BOTTOM) * PANEL_W) as usize..].iter().all(|&v| v == WHITE), "{}", b.id);
    }
}

#[test]
fn renders_are_deterministic_and_distinct() {
    let ids: Vec<&str> = BUILTINS.iter().map(|b| b.id).collect();
    assert_eq!(
        ids,
        ["blank", "lined", "dot_grid", "graph", "cornell", "daily", "weekly", "isometric", "music_staff"]
    );
    let mut hashes = Vec::new();
    for b in BUILTINS {
        let a = render(&b.spec, PANEL_W, PANEL_H);
        let again = render(&b.spec, PANEL_W, PANEL_H);
        assert_eq!(sha256_hex(&a), sha256_hex(&again), "{}", b.id);
        if b.id == "blank" {
            assert!(a.iter().all(|&v| v == WHITE));
        }
        hashes.push(sha256_hex(&a));
    }
    let mut sorted = hashes.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 9);
}

#[test]
fn lined_puts_rules_at_the_spacing_and_nowhere_else() {
    let spec = TemplateSpec::Lined { spacing_px: 94, margin_px: 160 };
    let px = render(&spec, PANEL_W, PANEL_H);
    let w = PANEL_W as usize;
    let row = |y: u32| &px[y as usize * w..(y as usize + 1) * w];
    let rule = row(SAFE_TOP + 94);
    assert!(rule[160..w - 160].iter().all(|&v| v == LINE));
    assert!(rule[..160].iter().all(|&v| v == WHITE));
    assert!(rule[w - 160..].iter().all(|&v| v == WHITE));
    assert!(row(SAFE_TOP + 47).iter().all(|&v| v == WHITE));
}

#[test]
fn thumbnail_box_averages_by_factor() {
    let mut px = vec![0u8; 256];
    for y in 0..16 {
        for x in 0..16 {
            px[y * 16 + x] = if (x + y) % 2 == 0 { 0xFF } else { 0x00 };
        }
    }
    let t = thumbnail(&px, 16, 16, 8);
    assert_eq!(t.len(), 4);
    assert!(t.iter().all(|&v| (0x7F..=0x81).contains(&v)), "{t:?}");
}

fn row_of(px: &[u8], y: u32) -> &[u8] {
    &px[y as usize * PANEL_W as usize..(y as usize + 1) * PANEL_W as usize]
}
fn at(px: &[u8], x: u32, y: u32) -> u8 {
    px[(y * PANEL_W + x) as usize]
}
fn builtin(id: &str) -> Vec<u8> {
    render(&BUILTINS.iter().find(|b| b.id == id).unwrap().spec, PANEL_W, PANEL_H)
}

#[test]
fn daily_has_a_header_rule_at_safe_top_plus_300() {
    let px = builtin("daily");
    assert!(row_of(&px, SAFE_TOP + 300)[160..(PANEL_W - 160) as usize].iter().all(|&v| v == LINE));
    assert!(row_of(&px, SAFE_TOP + 299).iter().all(|&v| v == WHITE));
}

#[test]
fn cornell_column_and_summary_rule_sit_where_specified() {
    let px = builtin("cornell");
    assert_eq!(at(&px, 560, 1000), LINE);
    assert_eq!(at(&px, 559, 1000), WHITE);
    let summary = PANEL_H - SAFE_BOTTOM - 420;
    assert!(row_of(&px, summary).iter().all(|&v| v == LINE));
    assert!(row_of(&px, summary - 1).iter().filter(|&&v| v == LINE).count() == 2);
}

#[test]
fn weekly_has_six_rules_between_seven_equal_bands() {
    let px = builtin("weekly");
    let band = (PANEL_H - SAFE_TOP - SAFE_BOTTOM) / 7;
    for i in 1..7 {
        assert!(row_of(&px, SAFE_TOP + i * band).iter().all(|&v| v == LINE), "{i}");
    }
    assert!(row_of(&px, SAFE_TOP + band / 2).iter().all(|&v| v == WHITE));
}

#[test]
fn music_staff_first_group_has_five_lines_24px_apart() {
    let px = builtin("music_staff");
    for k in 0..5 {
        assert_eq!(at(&px, 900, SAFE_TOP + 130 + 24 * k), LINE, "{k}");
        if k < 4 {
            assert_eq!(at(&px, 900, SAFE_TOP + 130 + 24 * k + 12), WHITE);
        }
    }
}

#[test]
fn dot_grid_dots_are_2x2_on_the_pitch() {
    let px = builtin("dot_grid");
    let (x, y) = (60, SAFE_TOP + 60);
    assert_eq!(at(&px, x, y), DOT);
    assert_eq!(at(&px, x + 1, y + 1), DOT);
    assert_eq!(at(&px, x + 3, y), WHITE);
}

#[test]
fn tiny_sizes_render_all_white_without_panicking() {
    for b in BUILTINS {
        for (w, h) in [(1, 1), (100, 100), (400, SAFE_TOP + SAFE_BOTTOM - 1)] {
            let px = render(&b.spec, w, h);
            assert_eq!(px.len(), (w * h) as usize);
            assert!(px.iter().all(|&v| v == WHITE), "{} {w}x{h}", b.id);
        }
    }
}

#[test]
fn huge_spec_values_do_not_overflow() {
    for spec in [
        TemplateSpec::Isometric { pitch_px: u32::MAX },
        TemplateSpec::DotGrid { pitch_px: u32::MAX },
        TemplateSpec::Graph { pitch_px: u32::MAX },
        TemplateSpec::Lined { spacing_px: u32::MAX, margin_px: u32::MAX },
    ] {
        assert_eq!(render(&spec, 300, 400).len(), 300 * 400);
    }
}

#[test]
fn spec_round_trips_through_serde_and_rejects_unknown_fields() {
    for b in BUILTINS {
        let json = serde_json::to_string(&b.spec).unwrap();
        assert_eq!(serde_json::from_str::<TemplateSpec>(&json).unwrap(), b.spec);
    }
    assert_eq!(
        serde_json::to_string(&TemplateSpec::DotGrid { pitch_px: 60 }).unwrap(),
        r#"{"kind":"dot_grid","pitch_px":60}"#
    );
    assert!(serde_json::from_str::<TemplateSpec>(r#"{"kind":"dot_grid","pitch_px":60,"extra":1}"#).is_err());
}

#[test]
fn thumbnail_of_a_short_bitmap_is_white_not_a_panic() {
    assert_eq!(thumbnail(&[0u8; 10], 16, 16, 8), vec![WHITE; 4]);
}
