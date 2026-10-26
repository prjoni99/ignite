use super::*;
use crate::nbk_test_kit::{geo, tmp};
use page_templates::{PANEL_H, PANEL_W, SAFE_BOTTOM, SAFE_TOP, WHITE};
use std::fs;

fn panel_geo() -> FbGeometry {
    FbGeometry {
        xres: PANEL_W,
        yres: PANEL_H,
        line_length: PANEL_W,
        ..geo()
    }
}

fn encode(
    w: u32,
    h: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    data: &[u8],
    tweak: impl FnOnce(&mut png::Encoder<&mut Vec<u8>>),
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        tweak(&mut enc);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(data).unwrap();
    }
    out
}

fn rgba_png(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
    let data: Vec<u8> = (0..w * h).flat_map(|_| px).collect();
    encode(
        w,
        h,
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        &data,
        |_| {},
    )
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// The png 0.17 encoder cannot interlace, so build an Adam7 8-bit grayscale PNG by hand
/// (stored deflate blocks).
fn adam7_gray_png(w: u32, h: u32, px: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    const PASSES: [(u32, u32, u32, u32); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    let mut raw = Vec::new();
    for (xs, ys, dx, dy) in PASSES {
        if xs >= w || ys >= h {
            continue;
        }
        let mut y = ys;
        while y < h {
            raw.push(0);
            let mut x = xs;
            while x < w {
                raw.push(px(x, y));
                x += dx;
            }
            y += dy;
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(60_000).peekable();
    while let Some(c) = chunks.next() {
        z.push(u8::from(chunks.peek().is_none()));
        z.extend((c.len() as u16).to_le_bytes());
        z.extend((!(c.len() as u16)).to_le_bytes());
        z.extend(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 0, 0, 0, 1]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

fn safe_rows_white(bm: &[u8]) {
    let w = PANEL_W as usize;
    assert_eq!(bm.len(), w * PANEL_H as usize);
    assert!(bm[..SAFE_TOP as usize * w].iter().all(|&v| v == WHITE));
    assert!(bm[(PANEL_H - SAFE_BOTTOM) as usize * w..]
        .iter()
        .all(|&v| v == WHITE));
}

fn put_custom(dir: &Path, name: &str, bytes: &[u8]) {
    let d = dir.join(TEMPLATES_DIR).join(CUSTOM_DIR);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join(name), bytes).unwrap();
}

fn custom(name: &str) -> TemplateId {
    TemplateId::Custom { name: name.into() }
}

#[test]
fn list_has_all_builtins_then_custom_files_sorted_with_unreadable_marked() {
    let dir = tmp("tpl-list");
    put_custom(&dir, "beta.png", &rgba_png(4, 4, [0, 0, 0, 255]));
    put_custom(&dir, "Alpha.png", &rgba_png(4, 4, [0, 0, 0, 255]));
    put_custom(&dir, "broken.png", b"not a png at all");
    put_custom(&dir, "notes.txt", b"ignored");
    let l = list(&dir);
    let n = page_templates::BUILTINS.len();
    assert_eq!(l.len(), n + 3);
    for (e, b) in l.iter().zip(page_templates::BUILTINS) {
        assert_eq!(e.id, TemplateId::Builtin { id: b.id.into() });
        assert_eq!(e.name, b.name);
        assert_eq!(e.status, TemplateStatus::Ready);
    }
    let names: Vec<_> = l[n..].iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Alpha", "beta", "broken"]);
    assert_eq!(l[n].status, TemplateStatus::Ready);
    assert!(matches!(l[n + 2].status, TemplateStatus::Unreadable(_)));
    assert_eq!(l[n].id, custom("Alpha"));
    // No custom dir at all: just builtins.
    assert_eq!(list(&tmp("tpl-list-empty")).len(), n);
}

#[test]
fn bitmap_renders_once_and_reuses_the_cache() {
    let dir = tmp("tpl-cache");
    let g = panel_geo();
    let id = TemplateId::Builtin { id: "lined".into() };
    let a = bitmap(&dir, &id, &g).unwrap();
    assert_eq!(a.len(), (PANEL_W * PANEL_H) as usize);
    safe_rows_white(&a);
    let cache = dir.join(TEMPLATES_DIR).join(CACHE_DIR);
    let full = |c: &Path| -> Vec<_> {
        fs::read_dir(c)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                n.ends_with(".gray8") && !n.ends_with(".thumb.gray8")
            })
            .collect()
    };
    let files = full(&cache);
    assert_eq!(files.len(), 1);
    assert!(cache.join("index.json").exists());
    let m1 = fs::metadata(&files[0]).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let b = bitmap(&dir, &id, &g).unwrap();
    assert_eq!(a, b);
    assert_eq!(full(&cache).len(), 1);
    assert_eq!(fs::metadata(&files[0]).unwrap().modified().unwrap(), m1);
    // Thumbnail is factor 8 and cached beside.
    let t = thumbnail(&dir, &id, &g).unwrap();
    assert_eq!(t.len(), (PANEL_W / 8 * (PANEL_H / 8)) as usize);
    assert_eq!(thumbnail(&dir, &id, &g).unwrap(), t);
    // Deleted cache file is re-rendered; corrupt index is rebuilt.
    fs::remove_file(&files[0]).unwrap();
    assert_eq!(bitmap(&dir, &id, &g).unwrap(), a);
    fs::write(cache.join("index.json"), b"{ garbage").unwrap();
    assert_eq!(bitmap(&dir, &id, &g).unwrap(), a);
    assert_eq!(full(&cache).len(), 1);
    // Unknown builtin is an error.
    assert!(bitmap(&dir, &TemplateId::Builtin { id: "nope".into() }, &g).is_err());
}

#[test]
fn custom_png_variants_decode_or_are_reported() {
    let dir = tmp("tpl-variants");
    let g = panel_geo();
    // 16-bit grayscale.
    let gray16: Vec<u8> = (0..8 * 8).flat_map(|_| [0x40u8, 0x00]).collect();
    let g16 = encode(
        8,
        8,
        png::ColorType::Grayscale,
        png::BitDepth::Sixteen,
        &gray16,
        |_| {},
    );
    // Indexed with a palette.
    let idx = encode(
        4,
        4,
        png::ColorType::Indexed,
        png::BitDepth::Eight,
        &[0u8; 16],
        |e| e.set_palette(vec![0, 0, 0, 255, 255, 255]),
    );
    // Adam7 interlaced grayscale (hand-built; the encoder cannot interlace).
    let adam = adam7_gray_png(16, 16, |x, y| ((x * 16 + y * 3) % 200) as u8);
    let rgba = rgba_png(10, 10, [10, 20, 30, 255]);
    let mid = |bm: &[u8]| {
        bm[((SAFE_TOP + (PANEL_H - SAFE_TOP - SAFE_BOTTOM) / 2) * PANEL_W + PANEL_W / 2) as usize]
    };
    // Indexed with a transparent entry (tRNS): index 1 is fully transparent.
    let trns = encode(
        4,
        4,
        png::ColorType::Indexed,
        png::BitDepth::Eight,
        &[1u8; 16],
        |e| {
            e.set_palette(vec![0, 0, 0, 0, 0, 0]);
            e.set_trns(vec![255, 0]);
        },
    );
    // Adam7 with a black centre block on white.
    let adam_block = adam7_gray_png(16, 16, |x, y| {
        if (4..12).contains(&x) && (4..12).contains(&y) {
            0
        } else {
            255
        }
    });
    let expect: [(&str, &Vec<u8>, u8); 6] = [
        ("rgba", &rgba, 0x12), // (77*10+150*20+29*30)>>8
        ("g16", &g16, 0x40),
        ("idx", &idx, 0x00),
        ("adam", &adam, 152), // (8*16+8*3)%200 at the source centre
        ("trns", &trns, 0xFF),
        ("block", &adam_block, 0x00),
    ];
    for (name, bytes, centre) in expect {
        put_custom(&dir, &format!("{name}.png"), bytes);
        let bm = bitmap(&dir, &custom(name), &g).unwrap_or_else(|e| panic!("{name}: {e}"));
        safe_rows_white(&bm);
        assert_eq!(mid(&bm), centre, "{name} centre pixel");
        assert_eq!(
            thumbnail(&dir, &custom(name), &g).unwrap().len(),
            (PANEL_W / 8 * (PANEL_H / 8)) as usize
        );
    }
    let adam_bm = bitmap(&dir, &custom("adam"), &g).unwrap();
    assert!(adam_bm.iter().any(|&v| v < 0xF0), "adam has ink");
    let trns_bm = bitmap(&dir, &custom("trns"), &g).unwrap();
    assert!(trns_bm.iter().all(|&v| v == WHITE), "transparent -> white");
    // Truncated: Unreadable in list, Err from bitmap, nothing cached for it.
    let cache = dir.join(TEMPLATES_DIR).join(CACHE_DIR);
    let before = fs::read_dir(&cache).unwrap().count();
    put_custom(&dir, "trunc.png", &adam[..adam.len() / 2]);
    let e = list(&dir).into_iter().find(|e| e.name == "trunc").unwrap();
    assert!(matches!(e.status, TemplateStatus::Unreadable(_)));
    let err = bitmap(&dir, &custom("trunc"), &g).unwrap_err();
    assert!(err.starts_with("REFUSE TEMPLATE"), "{err}");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), before);
    // Garbage, zero bytes and hostile names never panic.
    assert!(decode_custom_png(b"", PANEL_W, PANEL_H).is_err());
    assert!(decode_custom_png(&[0x89, b'P', b'N', b'G'], PANEL_W, PANEL_H).is_err());
    assert!(bitmap(&dir, &custom("../escape"), &g).is_err());
    assert!(bitmap(&dir, &custom("missing"), &g).is_err());
}

#[test]
fn custom_png_replaced_on_disk_is_redecoded() {
    let dir = tmp("tpl-replace");
    let g = panel_geo();
    let path = dir.join(TEMPLATES_DIR).join(CUSTOM_DIR).join("x.png");
    // Same dimensions and colour type => identical encoded length; only mtime can differ.
    put_custom(&dir, "x.png", &adam7_gray_png(8, 8, |_, _| 0));
    let a = bitmap(&dir, &custom("x"), &g).unwrap();
    let old_len = fs::metadata(&path).unwrap().len();
    put_custom(&dir, "x.png", &adam7_gray_png(8, 8, |_, _| 200));
    assert_eq!(fs::metadata(&path).unwrap().len(), old_len, "same length");
    let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(f);
    let b = bitmap(&dir, &custom("x"), &g).unwrap();
    assert_ne!(a, b);
}

#[test]
fn a_changed_builtin_spec_or_renderer_version_re_renders() {
    let dir = tmp("tpl-stamp");
    let g = panel_geo();
    let id = TemplateId::Builtin { id: "graph".into() };
    let a = bitmap(&dir, &id, &g).unwrap();
    let cache = dir.join(TEMPLATES_DIR).join(CACHE_DIR);
    let ipath = cache.join("index.json");
    let mut idx: serde_json::Value = serde_json::from_slice(&fs::read(&ipath).unwrap()).unwrap();
    let key = serde_json::to_string(&id).unwrap();
    let real_sha = idx["entries"][&key]["sha256"].as_str().unwrap().to_owned();
    assert!(!idx["entries"][&key]["spec_stamp"]
        .as_str()
        .unwrap()
        .is_empty());
    // Point at a bogus cached file with a wrong stamp: it must not be served.
    let bogus = "0".repeat(64);
    idx["entries"][&key]["sha256"] = bogus.clone().into();
    idx["entries"][&key]["spec_stamp"] = "stale".into();
    fs::write(cache.join(format!("{bogus}.gray8")), vec![0u8; a.len()]).unwrap();
    fs::write(&ipath, serde_json::to_vec(&idx).unwrap()).unwrap();
    let b = bitmap(&dir, &id, &g).unwrap();
    assert_eq!(a, b, "re-rendered, not the stale file");
    let idx: serde_json::Value = serde_json::from_slice(&fs::read(&ipath).unwrap()).unwrap();
    assert_eq!(idx["entries"][&key]["sha256"].as_str().unwrap(), real_sha);
    assert_ne!(
        idx["entries"][&key]["spec_stamp"].as_str().unwrap(),
        "stale"
    );
}

#[test]
fn extension_case_is_accepted_and_duplicates_are_marked() {
    let dir = tmp("tpl-ext");
    let g = panel_geo();
    put_custom(&dir, "Up.PNG", &rgba_png(4, 4, [0, 0, 0, 255]));
    assert!(bitmap(&dir, &custom("Up"), &g).is_ok());
    put_custom(&dir, "dup.png", &rgba_png(4, 4, [0, 0, 0, 255]));
    put_custom(&dir, "dup.PNG", &rgba_png(4, 4, [0, 0, 0, 255]));
    // Case-insensitive filesystems (macOS default) cannot hold both files.
    if fs::read_dir(dir.join(TEMPLATES_DIR).join(CUSTOM_DIR))
        .unwrap()
        .count()
        < 3
    {
        return;
    }
    let l = list(&dir);
    let dups: Vec<_> = l.iter().filter(|e| e.name == "dup").collect();
    assert_eq!(dups.len(), 2);
    assert_eq!(dups[0].status, TemplateStatus::Ready);
    assert_eq!(
        dups[1].status,
        TemplateStatus::Unreadable("duplicate name".into())
    );
}

#[test]
fn custom_png_is_letterboxed_into_the_safe_area() {
    let bm = decode_custom_png(&rgba_png(100, 300, [0, 0, 0, 255]), PANEL_W, PANEL_H).unwrap();
    let (w, h) = (PANEL_W as usize, PANEL_H as usize);
    let (top, bot) = (SAFE_TOP as usize, h - SAFE_BOTTOM as usize);
    let row = |y: usize| &bm[y * w..(y + 1) * w];
    assert!(bm[..top * w].iter().all(|&v| v == WHITE));
    assert!(bm[bot * w..].iter().all(|&v| v == WHITE));
    let mid = row((top + bot) / 2);
    let ink: Vec<usize> = (0..w).filter(|&x| mid[x] == 0).collect();
    assert!(!ink.is_empty());
    assert!(mid.iter().all(|&v| v == 0 || v == WHITE));
    let (l, r) = (*ink.first().unwrap(), *ink.last().unwrap());
    assert_eq!(r - l + 1, ink.len(), "contiguous column");
    assert!((l as i64 - (w - 1 - r) as i64).abs() <= 1, "centred");
    assert!(l > 0 && r < w - 1, "pillarboxed");
    // Column spans the whole safe height.
    assert_eq!(bm[top * w + w / 2], 0);
    assert_eq!(bm[(bot - 1) * w + w / 2], 0);
}

#[test]
fn translucent_pixels_composite_over_white() {
    let bm = decode_custom_png(&rgba_png(4, 4, [0, 0, 0, 0]), PANEL_W, PANEL_H).unwrap();
    assert!(bm.iter().all(|&v| v == WHITE));
}

#[test]
fn template_preference_round_trips_and_keeps_side_button_and_mirror() {
    let dir = tmp("tpl-prefs");
    let p = dir.join(crate::prefs::PREFERENCES_FILE);
    assert_eq!(
        crate::prefs::load_template(&p),
        TemplateId::Builtin { id: "blank".into() }
    );
    crate::prefs::save_side_button(&p, crate::prefs::SideButtonAction::CreateLink).unwrap();
    crate::prefs::save_mirror_enabled(&p, false).unwrap();
    let id = custom("mine");
    crate::prefs::save_template(&p, &id).unwrap();
    assert_eq!(crate::prefs::load_template(&p), id);
    assert_eq!(
        crate::prefs::load_side_button(&p),
        crate::prefs::SideButtonAction::CreateLink
    );
    assert!(!crate::prefs::load_mirror_enabled(&p));
    crate::prefs::save_mirror_enabled(&p, true).unwrap();
    assert_eq!(crate::prefs::load_template(&p), id);
    fs::write(
        &p,
        br#"{"v":1,"side_button":"create_link","template":{"kind":"wat"}}"#,
    )
    .unwrap();
    assert_eq!(
        crate::prefs::load_template(&p),
        TemplateId::Builtin { id: "blank".into() }
    );
    fs::write(&p, br#"{"v":1,"side_button":"create_link"}"#).unwrap();
    assert_eq!(
        crate::prefs::load_template(&p),
        TemplateId::Builtin { id: "blank".into() }
    );
}
