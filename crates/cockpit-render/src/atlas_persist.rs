//! Glyph atlas disk cache — v0.6 M6.4.
//!
//! The renderer warms up its glyph atlas as text is rendered; on a cold
//! start every glyph the cockpit paints is re-rasterized. This module
//! gives the atlas a persistent representation so the warmed state can
//! be reloaded next launch and pushed straight to the GPU instead.
//!
//! Two pieces:
//!
//! - [`AtlasSnapshot`] is the on-disk view: atlas dimensions, the
//!   raw RGBA8 pixel buffer, and a manifest mapping stable font keys
//!   (PostScript name + weight) plus glyph cache parameters to the
//!   rect / left / top placement.
//! - [`serialize`] / [`deserialize`] turn the snapshot into a small
//!   binary blob with a magic header + version so any layout change
//!   bumps the version and old caches are dropped cleanly. The format
//!   is hand-rolled rather than serde-bincode to keep the renderer
//!   dependency footprint small (spec §24 instant-load posture).
//!
//! The GPU upload step lives in `cockpit-render::renderer`; the
//! glyph-cache rehydration lives in `cockpit-render::glyph_cache`.
//! This module is pure data plus codec so it can be unit-tested
//! without a GL context, a font system, or a display server.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use cosmic_text::FontSystem;
use thiserror::Error;

/// Magic prefix on the disk format — eight ASCII bytes so it survives
/// hex-dump inspection. Any mismatch is treated as a foreign / corrupt
/// file and the cache is discarded.
pub const ATLAS_MAGIC: &[u8; 8] = b"COCKPATL";

/// On-disk format version. Bump whenever the layout changes — older
/// caches with a different version are silently dropped.
pub const ATLAS_FORMAT_VERSION: u32 = 1;

/// In-memory representation of one persisted glyph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphManifestEntry {
    /// Stable font identifier — PostScript name of the face the glyph
    /// belongs to. On rehydration the renderer looks the face up by
    /// name in the fresh `fontdb` and assigns the new session-local id.
    pub font_post_script_name: String,
    /// `fontdb::Weight` inner value at the time of rasterization. Stored
    /// alongside the font key so a future "weight-derived" lookup can
    /// disambiguate when the same family ships several weights.
    pub font_weight: u16,
    /// `cosmic_text::CacheKey::glyph_id`.
    pub glyph_id: u16,
    /// `cosmic_text::CacheKey::font_size_bits`.
    pub font_size_bits: u32,
    /// `cosmic_text::CacheKey::x_bin` as `0..=3`.
    pub x_bin: u8,
    /// `cosmic_text::CacheKey::y_bin` as `0..=3`.
    pub y_bin: u8,
    /// `cosmic_text::CacheKey::flags::bits()`.
    pub flags: u32,
    /// Atlas pixel rectangle the glyph occupies.
    pub rect_x: i32,
    /// Atlas pixel rectangle the glyph occupies.
    pub rect_y: i32,
    /// Atlas pixel rectangle width.
    pub rect_w: i32,
    /// Atlas pixel rectangle height.
    pub rect_h: i32,
    /// Horizontal bearing recorded by the rasterizer.
    pub left: i32,
    /// Vertical bearing recorded by the rasterizer.
    pub top: i32,
}

/// Complete persisted atlas state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasSnapshot {
    /// Atlas texture width in pixels.
    pub atlas_width: i32,
    /// Atlas texture height in pixels.
    pub atlas_height: i32,
    /// Hash of the font / theme configuration at write time. Compared
    /// against the live config hash on load so a fresh font set (new
    /// system font installed, theme change that swapped fonts, …)
    /// invalidates the cache without risking glyph mismatch.
    pub config_hash: u64,
    /// Manifest entries — one per cached glyph, in stable order.
    pub glyphs: Vec<GlyphManifestEntry>,
    /// Atlas pixel buffer, length `atlas_width * atlas_height * 4`.
    pub pixels: Vec<u8>,
}

impl AtlasSnapshot {
    /// Total byte size of `pixels` required for an `atlas_width × atlas_height`
    /// RGBA8 buffer. Returns `None` on overflow.
    pub fn pixel_buffer_size(atlas_width: i32, atlas_height: i32) -> Option<usize> {
        if atlas_width <= 0 || atlas_height <= 0 {
            return None;
        }
        let w = atlas_width as i64;
        let h = atlas_height as i64;
        let total = w.checked_mul(h)?.checked_mul(4)?;
        if total < 0 {
            return None;
        }
        usize::try_from(total).ok()
    }

    /// True when `pixels.len()` matches the declared dimensions.
    pub fn has_well_sized_pixel_buffer(&self) -> bool {
        Self::pixel_buffer_size(self.atlas_width, self.atlas_height) == Some(self.pixels.len())
    }
}

/// Snapshot codec error.
#[derive(Debug, Error)]
pub enum AtlasPersistError {
    /// I/O error reading or writing the on-disk buffer.
    #[error("atlas snapshot I/O: {0}")]
    Io(#[from] io::Error),
    /// The magic header didn't match — file is corrupt or not ours.
    #[error("atlas snapshot magic mismatch")]
    BadMagic,
    /// The version field doesn't match the version this build understands.
    #[error("atlas snapshot version {found} (expected {expected})")]
    BadVersion { found: u32, expected: u32 },
    /// A length field declared more bytes than the buffer actually held.
    #[error("atlas snapshot truncated: {what}")]
    Truncated { what: &'static str },
    /// An integer field carried an out-of-range value (e.g. a `SubpixelBin`
    /// outside `0..=3`).
    #[error("atlas snapshot value out of range: {what}")]
    OutOfRange { what: &'static str },
    /// Pixel buffer length didn't match the declared dimensions.
    #[error("atlas snapshot pixel buffer expected {expected} bytes, found {found}")]
    PixelBufferSize { expected: usize, found: usize },
}

/// Encode `snapshot` to a byte buffer.
pub fn serialize(snapshot: &AtlasSnapshot) -> Result<Vec<u8>, AtlasPersistError> {
    if !snapshot.has_well_sized_pixel_buffer() {
        let expected =
            AtlasSnapshot::pixel_buffer_size(snapshot.atlas_width, snapshot.atlas_height)
                .unwrap_or(0);
        return Err(AtlasPersistError::PixelBufferSize {
            expected,
            found: snapshot.pixels.len(),
        });
    }

    let glyph_count_u32: u32 =
        snapshot
            .glyphs
            .len()
            .try_into()
            .map_err(|_| AtlasPersistError::OutOfRange {
                what: "glyph count exceeds u32",
            })?;

    let mut out = Vec::with_capacity(estimate_size(snapshot));
    out.write_all(ATLAS_MAGIC)?;
    out.write_all(&ATLAS_FORMAT_VERSION.to_le_bytes())?;
    out.write_all(&snapshot.config_hash.to_le_bytes())?;
    out.write_all(&snapshot.atlas_width.to_le_bytes())?;
    out.write_all(&snapshot.atlas_height.to_le_bytes())?;
    out.write_all(&glyph_count_u32.to_le_bytes())?;

    for glyph in &snapshot.glyphs {
        write_glyph(&mut out, glyph)?;
    }

    out.write_all(&snapshot.pixels)?;
    Ok(out)
}

/// Decode a byte buffer back into an [`AtlasSnapshot`]. Returns a typed
/// error on any structural problem so callers can drop the cache silently.
pub fn deserialize(bytes: &[u8]) -> Result<AtlasSnapshot, AtlasPersistError> {
    let mut cursor = io::Cursor::new(bytes);

    let mut magic = [0u8; 8];
    cursor
        .read_exact(&mut magic)
        .map_err(|_| AtlasPersistError::Truncated { what: "magic" })?;
    if &magic != ATLAS_MAGIC {
        return Err(AtlasPersistError::BadMagic);
    }

    let version = read_u32(&mut cursor, "version")?;
    if version != ATLAS_FORMAT_VERSION {
        return Err(AtlasPersistError::BadVersion {
            found: version,
            expected: ATLAS_FORMAT_VERSION,
        });
    }

    let config_hash = read_u64(&mut cursor, "config_hash")?;
    let atlas_width = read_i32(&mut cursor, "atlas_width")?;
    let atlas_height = read_i32(&mut cursor, "atlas_height")?;
    if atlas_width <= 0 || atlas_height <= 0 {
        return Err(AtlasPersistError::OutOfRange {
            what: "non-positive atlas dimension",
        });
    }
    let glyph_count = read_u32(&mut cursor, "glyph_count")?;

    let mut glyphs = Vec::with_capacity(glyph_count as usize);
    for _ in 0..glyph_count {
        glyphs.push(read_glyph(&mut cursor)?);
    }

    let expected = AtlasSnapshot::pixel_buffer_size(atlas_width, atlas_height).ok_or(
        AtlasPersistError::OutOfRange {
            what: "pixel buffer size overflow",
        },
    )?;
    let mut pixels = vec![0u8; expected];
    cursor
        .read_exact(&mut pixels)
        .map_err(|_| AtlasPersistError::Truncated {
            what: "pixel buffer",
        })?;

    Ok(AtlasSnapshot {
        atlas_width,
        atlas_height,
        config_hash,
        glyphs,
        pixels,
    })
}

fn estimate_size(snapshot: &AtlasSnapshot) -> usize {
    // Header (8 magic + 4 version + 8 hash + 4 + 4 + 4) = 32 bytes,
    // plus pixel buffer, plus a slack 40 bytes per manifest entry.
    32 + snapshot.pixels.len() + snapshot.glyphs.len() * 40
}

fn write_glyph(out: &mut Vec<u8>, glyph: &GlyphManifestEntry) -> Result<(), AtlasPersistError> {
    let name_bytes = glyph.font_post_script_name.as_bytes();
    let name_len: u16 = name_bytes
        .len()
        .try_into()
        .map_err(|_| AtlasPersistError::OutOfRange {
            what: "font post-script name longer than u16",
        })?;
    out.write_all(&name_len.to_le_bytes())?;
    out.write_all(name_bytes)?;
    out.write_all(&glyph.font_weight.to_le_bytes())?;
    out.write_all(&glyph.glyph_id.to_le_bytes())?;
    out.write_all(&glyph.font_size_bits.to_le_bytes())?;
    out.write_all(&[glyph.x_bin, glyph.y_bin])?;
    out.write_all(&glyph.flags.to_le_bytes())?;
    out.write_all(&glyph.rect_x.to_le_bytes())?;
    out.write_all(&glyph.rect_y.to_le_bytes())?;
    out.write_all(&glyph.rect_w.to_le_bytes())?;
    out.write_all(&glyph.rect_h.to_le_bytes())?;
    out.write_all(&glyph.left.to_le_bytes())?;
    out.write_all(&glyph.top.to_le_bytes())?;
    Ok(())
}

fn read_glyph(cursor: &mut io::Cursor<&[u8]>) -> Result<GlyphManifestEntry, AtlasPersistError> {
    let name_len = read_u16(cursor, "font name length")? as usize;
    let mut name_bytes = vec![0u8; name_len];
    cursor
        .read_exact(&mut name_bytes)
        .map_err(|_| AtlasPersistError::Truncated {
            what: "font post-script name",
        })?;
    let font_post_script_name =
        String::from_utf8(name_bytes).map_err(|_| AtlasPersistError::OutOfRange {
            what: "font post-script name not utf-8",
        })?;
    let font_weight = read_u16(cursor, "font weight")?;
    let glyph_id = read_u16(cursor, "glyph id")?;
    let font_size_bits = read_u32(cursor, "font size")?;
    let mut bins = [0u8; 2];
    cursor
        .read_exact(&mut bins)
        .map_err(|_| AtlasPersistError::Truncated {
            what: "subpixel bins",
        })?;
    let x_bin = bins[0];
    let y_bin = bins[1];
    if x_bin > 3 || y_bin > 3 {
        return Err(AtlasPersistError::OutOfRange {
            what: "subpixel bin not in 0..=3",
        });
    }
    let flags = read_u32(cursor, "flags")?;
    let rect_x = read_i32(cursor, "rect_x")?;
    let rect_y = read_i32(cursor, "rect_y")?;
    let rect_w = read_i32(cursor, "rect_w")?;
    let rect_h = read_i32(cursor, "rect_h")?;
    let left = read_i32(cursor, "left")?;
    let top = read_i32(cursor, "top")?;
    Ok(GlyphManifestEntry {
        font_post_script_name,
        font_weight,
        glyph_id,
        font_size_bits,
        x_bin,
        y_bin,
        flags,
        rect_x,
        rect_y,
        rect_w,
        rect_h,
        left,
        top,
    })
}

fn read_u16(cursor: &mut io::Cursor<&[u8]>, what: &'static str) -> Result<u16, AtlasPersistError> {
    let mut buf = [0u8; 2];
    cursor
        .read_exact(&mut buf)
        .map_err(|_| AtlasPersistError::Truncated { what })?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(cursor: &mut io::Cursor<&[u8]>, what: &'static str) -> Result<u32, AtlasPersistError> {
    let mut buf = [0u8; 4];
    cursor
        .read_exact(&mut buf)
        .map_err(|_| AtlasPersistError::Truncated { what })?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(cursor: &mut io::Cursor<&[u8]>, what: &'static str) -> Result<u64, AtlasPersistError> {
    let mut buf = [0u8; 8];
    cursor
        .read_exact(&mut buf)
        .map_err(|_| AtlasPersistError::Truncated { what })?;
    Ok(u64::from_le_bytes(buf))
}

fn read_i32(cursor: &mut io::Cursor<&[u8]>, what: &'static str) -> Result<i32, AtlasPersistError> {
    let mut buf = [0u8; 4];
    cursor
        .read_exact(&mut buf)
        .map_err(|_| AtlasPersistError::Truncated { what })?;
    Ok(i32::from_le_bytes(buf))
}

/// Hash of every font face currently loaded in `font_system`'s database,
/// mixed with the atlas dimensions and padding. Used as the
/// [`AtlasSnapshot::config_hash`] both at write (snapshot time) and at
/// read (load-and-validate time) — any mismatch invalidates the disk
/// cache and the renderer rebuilds the atlas from scratch.
///
/// Hash inputs (in order): atlas_width, atlas_height, padding, then the
/// sorted PostScript names of every face. PostScript names are stable
/// per font file, so the same font set hashes the same across launches.
pub fn font_set_config_hash(
    font_system: &FontSystem,
    atlas_width: i32,
    atlas_height: i32,
    padding: i32,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    atlas_width.hash(&mut hasher);
    atlas_height.hash(&mut hasher);
    padding.hash(&mut hasher);
    let mut names: Vec<&str> = font_system
        .db()
        .faces()
        .map(|face| face.post_script_name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        name.hash(&mut hasher);
    }
    hasher.finish()
}

/// Resolve the OS-specific path where the cached glyph atlas should
/// live. Returns `None` when no cache directory is available (no `$HOME`,
/// CI sandbox, …) — callers treat that as "disable the disk cache".
pub fn default_cache_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "CodingCockpit", "cockpit")?;
    Some(
        dirs.cache_dir()
            .join(format!("glyph-atlas-v{ATLAS_FORMAT_VERSION}.bin")),
    )
}

/// Read and decode a snapshot from `path`. Returns `Ok(None)` if the
/// file does not exist; surfaces every other error to the caller.
pub fn load_from_disk(path: &Path) -> Result<Option<AtlasSnapshot>, AtlasPersistError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(AtlasPersistError::Io(err)),
    };
    deserialize(&bytes).map(Some)
}

/// Encode `snapshot` and write it to `path`. Creates parent directories
/// as needed; writes atomically via a tempfile + rename so a crash mid-
/// write doesn't corrupt the cache.
pub fn store_to_disk(path: &Path, snapshot: &AtlasSnapshot) -> Result<(), AtlasPersistError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serialize(snapshot)?;
    let tmp = path.with_extension("bin.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with(glyphs: Vec<GlyphManifestEntry>) -> AtlasSnapshot {
        let w = 8;
        let h = 4;
        let len = AtlasSnapshot::pixel_buffer_size(w, h).unwrap();
        let pixels: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        AtlasSnapshot {
            atlas_width: w,
            atlas_height: h,
            config_hash: 0xdead_beef_cafe_babe,
            glyphs,
            pixels,
        }
    }

    fn sample_glyph(name: &str) -> GlyphManifestEntry {
        GlyphManifestEntry {
            font_post_script_name: name.to_string(),
            font_weight: 400,
            glyph_id: 42,
            font_size_bits: 14.0f32.to_bits(),
            x_bin: 1,
            y_bin: 2,
            flags: 0,
            rect_x: 1,
            rect_y: 2,
            rect_w: 5,
            rect_h: 6,
            left: -1,
            top: 11,
        }
    }

    #[test]
    fn pixel_buffer_size_matches_dimensions() {
        assert_eq!(AtlasSnapshot::pixel_buffer_size(2, 3), Some(24));
        assert_eq!(AtlasSnapshot::pixel_buffer_size(0, 4), None);
        assert_eq!(AtlasSnapshot::pixel_buffer_size(-1, 4), None);
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let glyphs = vec![sample_glyph("Iosevka-Regular"), sample_glyph("Roboto-Bold")];
        let original = snapshot_with(glyphs);

        let bytes = serialize(&original).expect("serialize");
        let restored = deserialize(&bytes).expect("deserialize");

        assert_eq!(restored, original);
    }

    #[test]
    fn round_trip_handles_an_empty_manifest() {
        let original = snapshot_with(Vec::new());

        let bytes = serialize(&original).expect("serialize empty");
        let restored = deserialize(&bytes).expect("deserialize empty");

        assert_eq!(restored, original);
        assert!(restored.glyphs.is_empty());
    }

    #[test]
    fn round_trip_handles_long_font_names() {
        let long_name = "X".repeat(4096);
        let original = snapshot_with(vec![sample_glyph(&long_name)]);

        let bytes = serialize(&original).expect("serialize long name");
        let restored = deserialize(&bytes).expect("deserialize long name");

        assert_eq!(restored.glyphs[0].font_post_script_name.len(), 4096);
    }

    #[test]
    fn deserialize_rejects_a_foreign_magic_header() {
        let bogus = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        match deserialize(&bogus) {
            Err(AtlasPersistError::BadMagic) => {}
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_rejects_an_older_version() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(ATLAS_MAGIC);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        match deserialize(&bytes) {
            Err(AtlasPersistError::BadVersion { found: 0, expected }) => {
                assert_eq!(expected, ATLAS_FORMAT_VERSION);
            }
            other => panic!("expected BadVersion, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_rejects_a_truncated_buffer() {
        let original = snapshot_with(vec![sample_glyph("X")]);
        let bytes = serialize(&original).unwrap();
        let truncated = &bytes[..bytes.len() - 10];
        match deserialize(truncated) {
            Err(AtlasPersistError::Truncated { .. }) => {}
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_rejects_an_out_of_range_subpixel_bin() {
        let mut glyph = sample_glyph("Y");
        glyph.x_bin = 4;
        let snap = snapshot_with(vec![glyph]);
        let bytes = serialize(&snap).unwrap();
        match deserialize(&bytes) {
            Err(AtlasPersistError::OutOfRange { what }) => {
                assert!(what.contains("subpixel"), "what: {what}");
            }
            other => panic!("expected OutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn serialize_rejects_a_mismatched_pixel_buffer() {
        let mut snap = snapshot_with(Vec::new());
        snap.pixels.pop();
        match serialize(&snap) {
            Err(AtlasPersistError::PixelBufferSize { .. }) => {}
            other => panic!("expected PixelBufferSize, got {other:?}"),
        }
    }

    #[test]
    fn store_then_load_round_trips_through_a_tempfile() {
        let original = snapshot_with(vec![sample_glyph("Iosevka-Regular")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("atlas.bin");

        store_to_disk(&path, &original).expect("store");
        let loaded = load_from_disk(&path).expect("load").expect("present");

        assert_eq!(loaded, original);
        // The tempfile sibling must have been renamed away.
        assert!(!path.with_extension("bin.tmp").exists());
    }

    #[test]
    fn load_from_disk_returns_none_for_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-there.bin");
        assert!(load_from_disk(&path).expect("load").is_none());
    }

    #[test]
    fn load_from_disk_surfaces_corruption() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("garbage.bin");
        fs::write(&path, b"not-a-cockpit-atlas").expect("write garbage");
        match load_from_disk(&path) {
            Err(AtlasPersistError::BadMagic) => {}
            other => panic!("expected BadMagic for garbage file, got {other:?}"),
        }
    }

    #[test]
    fn font_set_config_hash_changes_with_atlas_size_and_padding() {
        let font_system = FontSystem::new();
        let baseline = font_set_config_hash(&font_system, 1024, 1024, 1);
        assert_ne!(baseline, font_set_config_hash(&font_system, 1024, 1024, 2));
        assert_ne!(baseline, font_set_config_hash(&font_system, 512, 1024, 1));
        assert_eq!(baseline, font_set_config_hash(&font_system, 1024, 1024, 1));
    }
}
