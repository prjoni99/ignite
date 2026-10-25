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
