//! Kindle-side page-template store: a cache of rendered/decoded template bitmaps under the
//! library root, custom PNG decoding, a listing, and (in `prefs.rs`) the last-used template.
//!
//! Layout under the library root (`--data-dir`):
//!
//! ```text
//! templates/cache/index.json          { "v": 1, "entries": { "<TemplateId json>": {sha256,src_len,src_mtime_ms} } }
//! templates/cache/<sha256>.gray8      full-panel bitmap (xres * yres, tightly packed)
//! templates/cache/<sha256>.thumb.gray8  box-averaged thumbnail, factor 8
//! templates/custom/<name>.png         user-supplied backgrounds
//! ```
//!
//! This module is only called from command handlers (create/insert flows), never from the pen
//! path. All cache writes are atomic (temp + rename + dir fsync); a corrupt or missing cache is
//! rebuilt and never fails a request.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use kindle_platform::FbGeometry;
use page_templates::{sha256_hex, BUILTINS, SAFE_BOTTOM, SAFE_TOP, WHITE};
use serde::{Deserialize, Serialize};

use crate::nbk_import::write_atomic;

pub const TEMPLATES_DIR: &str = "templates";
pub const CACHE_DIR: &str = "cache";
pub const CUSTOM_DIR: &str = "custom";

const INDEX_FILE: &str = "index.json";
const INDEX_VERSION: u32 = 1;
const THUMB_FACTOR: u32 = 8;
/// Refuse absurd PNGs before allocating for them.
const MAX_PNG_DIM: u32 = 16_384;
const MAX_PNG_PIXELS: u64 = 40_000_000;
const PNG_IEND: [u8; 12] = [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplateId {
    Builtin {
        id: String,
    },
    /// `name` is the file stem of `templates/custom/<name>.png`.
    Custom {
        name: String,
    },
}

impl Default for TemplateId {
    fn default() -> Self {
        TemplateId::Builtin { id: "blank".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateStatus {
    Ready,
    Unreadable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateEntry {
    pub id: TemplateId,
    pub name: String,
    pub status: TemplateStatus,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct IndexEntry {
    sha256: String,
    #[serde(default)]
    src_len: u64,
    #[serde(default)]
    src_mtime_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexFile {
    v: u32,
    entries: BTreeMap<String, IndexEntry>,
}

impl Default for IndexFile {
    fn default() -> Self {
        Self {
            v: INDEX_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(TEMPLATES_DIR).join(CACHE_DIR)
}

fn custom_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(TEMPLATES_DIR).join(CUSTOM_DIR)
}

fn refuse(msg: impl std::fmt::Display) -> String {
    format!("REFUSE TEMPLATE: {msg}")
}

/// Built-ins in `BUILTINS` order, then `templates/custom/*.png` by file name (case-insensitive).
/// Never fails: an unreadable custom directory yields only the built-ins.
pub fn list(data_dir: &Path) -> Vec<TemplateEntry> {
    let mut out: Vec<TemplateEntry> = BUILTINS
        .iter()
        .map(|b| TemplateEntry {
            id: TemplateId::Builtin { id: b.id.into() },
            name: b.name.into(),
            status: TemplateStatus::Ready,
        })
        .collect();
    let Ok(rd) = fs::read_dir(custom_dir(data_dir)) else {
        return out;
    };
    let mut files: Vec<(String, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let is_png = path
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("png"));
            let file_name = path.file_name()?.to_str()?.to_owned();
            (is_png && path.is_file()).then_some((file_name, path))
        })
        .collect();
    files.sort_by_key(|(n, _)| (n.to_lowercase(), n.clone()));
    for (_, path) in files {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_custom_name(stem) {
            continue;
        }
        let status = match probe_png(&path) {
            Ok(()) => TemplateStatus::Ready,
            Err(e) => TemplateStatus::Unreadable(e),
        };
        out.push(TemplateEntry {
            id: TemplateId::Custom { name: stem.into() },
            name: stem.into(),
            status,
        });
    }
    out
}

/// Cheap readability check: the PNG header parses and the file ends with an IEND chunk.
fn probe_png(path: &Path) -> Result<(), String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    {
        let dec = png::Decoder::new(std::io::BufReader::new(&mut f));
        let reader = dec.read_info().map_err(|e| e.to_string())?;
        let info = reader.info();
        if info.width == 0 || info.height == 0 {
            return Err("zero-sized image".into());
        }
    }
    if len < PNG_IEND.len() as u64 {
        return Err("truncated".into());
    }
    f.seek(SeekFrom::End(-(PNG_IEND.len() as i64)))
        .map_err(|e| e.to_string())?;
    let mut tail = [0u8; 12];
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    if tail != PNG_IEND {
        return Err("truncated".into());
    }
    Ok(())
}

fn valid_custom_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && !name.contains("..")
}

fn custom_path(data_dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_custom_name(name) {
        return Err(refuse(format!("invalid template name {name:?}")));
    }
    let dir = custom_dir(data_dir);
    for ext in ["png", "PNG"] {
        let p = dir.join(format!("{name}.{ext}"));
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(refuse(format!("custom template {name:?} not found")))
}

/// Full-panel gray8 bitmap (`geo.xres * geo.yres`, tightly packed) for `id`, cached by the sha256
/// of its bytes. Writes the cache atomically.
pub fn bitmap(data_dir: &Path, id: &TemplateId, geo: &FbGeometry) -> Result<Vec<u8>, String> {
    resolve(data_dir, id, geo).map(|(bytes, _)| bytes)
}

/// Factor-8 thumbnail of [`bitmap`], cached beside it.
pub fn thumbnail(data_dir: &Path, id: &TemplateId, geo: &FbGeometry) -> Result<Vec<u8>, String> {
    let (bytes, sha) = resolve(data_dir, id, geo)?;
    let (w, h) = (geo.xres, geo.yres);
    let want = (w / THUMB_FACTOR) as usize * (h / THUMB_FACTOR) as usize;
    let path = cache_dir(data_dir).join(format!("{sha}.thumb.gray8"));
    if let Ok(t) = fs::read(&path) {
        if t.len() == want {
            return Ok(t);
        }
    }
    let t = page_templates::thumbnail(&bytes, w, h, THUMB_FACTOR);
    write_atomic(&path, &t)?;
    Ok(t)
}

fn resolve(
    data_dir: &Path,
    id: &TemplateId,
    geo: &FbGeometry,
) -> Result<(Vec<u8>, String), String> {
    let (w, h) = (geo.xres, geo.yres);
    let want = w as usize * h as usize;
    let key = serde_json::to_string(id).map_err(|e| e.to_string())?;
    let cache = cache_dir(data_dir);
    let mut index = load_index(&cache);

    // Validate the id and capture the source stamp (custom only) before consulting the cache.
    let (src_path, src_len, src_mtime_ms) = match id {
        TemplateId::Builtin { id } => {
            if !BUILTINS.iter().any(|b| b.id == id) {
                return Err(refuse(format!("unknown built-in template {id:?}")));
            }
            (None, 0, 0)
        }
        TemplateId::Custom { name } => {
            let p = custom_path(data_dir, name)?;
            let md = fs::metadata(&p).map_err(|e| refuse(format!("{}: {e}", p.display())))?;
            let ms = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            (Some(p), md.len(), ms)
        }
    };

    if let Some(e) = index.entries.get(&key) {
        if e.src_len == src_len && e.src_mtime_ms == src_mtime_ms && sha_is_plausible(&e.sha256) {
            if let Ok(bytes) = fs::read(cache.join(format!("{}.gray8", e.sha256))) {
                if bytes.len() == want {
                    return Ok((bytes, e.sha256.clone()));
                }
            }
        }
    }

    let bytes = match (id, &src_path) {
        (TemplateId::Builtin { id }, _) => {
            let b = BUILTINS
                .iter()
                .find(|b| b.id == id)
                .expect("validated above");
            page_templates::render(&b.spec, w, h)
        }
        (TemplateId::Custom { .. }, Some(p)) => {
            let raw = fs::read(p).map_err(|e| refuse(format!("{}: {e}", p.display())))?;
            decode_custom_png(&raw, w, h)?
        }
        _ => return Err(refuse("unresolvable template")),
    };
    let sha = sha256_hex(&bytes);
    write_atomic(&cache.join(format!("{sha}.gray8")), &bytes)?;
    let thumb = page_templates::thumbnail(&bytes, w, h, THUMB_FACTOR);
    write_atomic(&cache.join(format!("{sha}.thumb.gray8")), &thumb)?;
    // Re-read so a concurrent update to other keys is not clobbered more than necessary.
    index = load_index(&cache);
    index.entries.insert(
        key,
        IndexEntry {
            sha256: sha.clone(),
            src_len,
            src_mtime_ms,
        },
    );
    match serde_json::to_vec_pretty(&index) {
        Ok(j) => write_atomic(&cache.join(INDEX_FILE), &j)?,
        Err(e) => return Err(e.to_string()),
    }
    Ok((bytes, sha))
}

fn sha_is_plausible(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Missing or corrupt index → empty index (logged once, never an error).
fn load_index(cache: &Path) -> IndexFile {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    let path = cache.join(INDEX_FILE);
    let Ok(raw) = fs::read(&path) else {
        return IndexFile::default();
    };
    match serde_json::from_slice::<IndexFile>(&raw) {
        Ok(i) if i.v == INDEX_VERSION => i,
        _ => {
            if !LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!("templates: cache index unreadable, rebuilding");
            }
            IndexFile::default()
        }
    }
}

/// Decode a PNG to a `w * h` gray8 page: luma (alpha composited over white), scaled with
/// nearest-neighbour to fit the safe area (`w x (h - SAFE_TOP - SAFE_BOTTOM)`) preserving aspect
/// ratio, centred, white everywhere else. Never panics on malformed input.
pub fn decode_custom_png(bytes: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec
        .read_info()
        .map_err(|e| refuse(format!("decode: {e}")))?;
    let (iw, ih) = {
        let i = reader.info();
        (i.width, i.height)
    };
    if iw == 0 || ih == 0 {
        return Err(refuse("zero-sized image"));
    }
    if iw > MAX_PNG_DIM || ih > MAX_PNG_DIM || iw as u64 * ih as u64 > MAX_PNG_PIXELS {
        return Err(refuse("image too large"));
    }
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| refuse(format!("decode: {e}")))?;
    let (channels, has_alpha, is_gray) = match frame.color_type {
        png::ColorType::Grayscale => (1usize, false, true),
        png::ColorType::GrayscaleAlpha => (2, true, true),
        png::ColorType::Rgb => (3, false, false),
        png::ColorType::Rgba => (4, true, false),
        png::ColorType::Indexed => return Err(refuse("unexpanded palette")),
    };
    if frame.bit_depth != png::BitDepth::Eight {
        return Err(refuse("unsupported bit depth"));
    }
    let (iw, ih) = (frame.width as usize, frame.height as usize);
    if buf.len() < frame.buffer_size() || frame.buffer_size() < iw * ih * channels {
        return Err(refuse("short image data"));
    }
    let luma: Vec<u8> = buf[..iw * ih * channels]
        .chunks_exact(channels)
        .map(|px| {
            let (y, a) = if is_gray {
                (px[0] as u32, if has_alpha { px[1] as u32 } else { 255 })
            } else {
                let y = (77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32) >> 8;
                (y, if has_alpha { px[3] as u32 } else { 255 })
            };
            ((y * a + 255 * (255 - a) + 127) / 255).min(255) as u8
        })
        .collect();

    let mut out = vec![WHITE; w as usize * h as usize];
    let area_w = w as u64;
    let area_h = h.saturating_sub(SAFE_TOP + SAFE_BOTTOM) as u64;
    if area_w == 0 || area_h == 0 {
        return Ok(out);
    }
    let (iw64, ih64) = (iw as u64, ih as u64);
    let (dw, dh) = if iw64 * area_h >= ih64 * area_w {
        (area_w, (ih64 * area_w / iw64).max(1))
    } else {
        ((iw64 * area_h / ih64).max(1), area_h)
    };
    let x0 = (area_w - dw) / 2;
    let y0 = SAFE_TOP as u64 + (area_h - dh) / 2;
    for dy in 0..dh {
        let sy = (dy * ih64 / dh).min(ih64 - 1) as usize;
        let src_row = &luma[sy * iw..(sy + 1) * iw];
        let o = ((y0 + dy) * w as u64 + x0) as usize;
        for dx in 0..dw {
            let sx = (dx * iw64 / dw).min(iw64 - 1) as usize;
            out[o + dx as usize] = src_row[sx];
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "templates_tests.rs"]
mod tests;
