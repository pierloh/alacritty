//! Single-pass text shaping via rustybuzz for ligature support.
//!
//! Shapes each line once with user-configured OpenType features. Uses cluster
//! boundaries from rustybuzz output to determine cell spans:
//! - 1 input char per cluster: texture healing (glyph ID substitution, span=1)
//! - N input chars per cluster: ligature (first cell gets glyph, rest are None)

use std::collections::HashMap;
use std::sync::Arc;

use crossfont::{FontKey, Rasterize};
use log::warn;
use rustybuzz::{Face, Feature, UnicodeBuffer};

/// A shaped glyph from rustybuzz.
#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    /// Glyph index in the font face.
    pub glyph_id: u32,
    /// Column span of this glyph (1 for normal, 2+ for ligatures).
    pub cell_span: u8,
    /// True if this glyph is part of a multi-glyph ligature (shared cluster).
    /// False for standalone substitutions like texture healing.
    pub is_ligature: bool,
}

/// Shaped output for a single line.
#[derive(Clone, Debug)]
pub struct ShapedLine {
    /// One entry per cell column. `Some` for the head cell of each glyph,
    /// `None` for continuation cells within a ligature or unshaped cells.
    pub glyphs: Vec<Option<ShapedGlyph>>,
}

/// Maximum shape cache entries before eviction. Bounded to prevent unbounded
/// growth from scrolling through large files. Typical visible lines < 100,
/// so 512 gives generous headroom for recently scrolled content.
const MAX_SHAPE_CACHE_ENTRIES: usize = 512;

/// Single-pass text shaper wrapping a rustybuzz Face.
///
/// Note: shaping uses the regular font face only. Shaped glyph IDs are
/// font-face-specific, so bold/italic cells fall back to character-based
/// lookup. This is correct for most ligature fonts (which share glyph tables
/// across weights) but may produce wrong glyphs for fonts with divergent
/// bold glyph tables.
pub struct Shaper {
    face_data: Arc<[u8]>,
    features: Vec<Feature>,
    cache: HashMap<Vec<char>, ShapedLine>,
}

impl Shaper {
    /// Create a new shaper from rasterizer font data.
    pub fn new<R: Rasterize>(
        rasterizer: &R,
        font_key: FontKey,
        feature_tags: &[String],
    ) -> Option<Self> {
        let face_data = rasterizer.font_data(font_key)?;
        Self::from_data(face_data, feature_tags)
    }

    /// Create a shaper directly from raw font bytes.
    pub fn from_data(face_data: Arc<[u8]>, feature_tags: &[String]) -> Option<Self> {
        // Validate that rustybuzz can parse this font.
        Face::from_slice(&face_data, 0)?;

        let features = Self::parse_features(feature_tags);
        if features.is_empty() {
            return None;
        }

        Some(Self { face_data, features, cache: HashMap::with_capacity(128) })
    }

    /// Parse feature tag strings (e.g. "calt", "liga") into rustybuzz Features.
    fn parse_features(tags: &[String]) -> Vec<Feature> {
        tags.iter()
            .filter_map(|tag| {
                let bytes = tag.as_bytes();
                if bytes.len() != 4 {
                    warn!("Ignoring invalid OpenType feature tag: {:?}", tag);
                    return None;
                }
                let tag = rustybuzz::ttf_parser::Tag::from_bytes(
                    bytes.try_into().ok()?,
                );
                Some(Feature::new(tag, 1, ..))
            })
            .collect()
    }

    /// Clear the shape cache (call on font reload/resize).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Shape a line of characters and return shaped glyph info per column.
    ///
    /// Returns `None` if the font can't be parsed. Unshaped cells (no feature
    /// effect) get `None` in the output -- the renderer falls back to
    /// character-based lookup for those.
    pub fn shape_line(&mut self, chars: &[char]) -> Option<ShapedLine> {
        if chars.is_empty() {
            return Some(ShapedLine { glyphs: Vec::new() });
        }

        // Cache lookup by actual content (no hash collisions).
        if let Some(cached) = self.cache.get(chars) {
            return Some(cached.clone());
        }

        let face = Face::from_slice(&self.face_data, 0)?;
        let text: String = chars.iter().collect();

        // Build byte-offset → column-index mapping for cluster resolution.
        let byte_to_col: Vec<usize> = {
            let mut map = Vec::new();
            for (col, c) in text.chars().enumerate() {
                for _ in 0..c.len_utf8() {
                    map.push(col);
                }
            }
            map
        };

        // Shape once with configured features.
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(&text);
        let output = rustybuzz::shape(&face, &self.features, buffer);
        let infos = output.glyph_infos();

        let num_cols = chars.len();
        let mut glyphs: Vec<Option<ShapedGlyph>> = vec![None; num_cols];

        if infos.len() == num_cols {
            // 1:1 mapping: glyph count matches char count. Map glyph[i] to
            // column[i] directly, but only emit glyphs where shaping changed
            // the glyph ID. Unchanged characters keep GlyphId::Char which
            // preserves fallback font lookup.
            let face_ref = face.as_ref();
            let mut changed = vec![false; num_cols];
            for (i, info) in infos.iter().enumerate() {
                if i < num_cols {
                    let default_gid = face_ref
                        .glyph_index(chars[i])
                        .map(|g| g.0 as u32)
                        .unwrap_or(0);
                    changed[i] = info.glyph_id != default_gid;
                }
            }

            // Detect ligature members: glyphs that share a cluster value with
            // an adjacent glyph. Texture healing has unique clusters per glyph.
            let mut is_lig = vec![false; num_cols];
            for i in 0..num_cols {
                if i + 1 < num_cols && infos[i].cluster == infos[i + 1].cluster {
                    is_lig[i] = true;
                    is_lig[i + 1] = true;
                }
            }

            // Expand changed regions: if col N changed, also keep col N-1 and
            // N+1 if they also changed. This preserves full ligature groups.
            let mut emit = vec![false; num_cols];
            for i in 0..num_cols {
                if changed[i] {
                    emit[i] = true;
                    if i > 0 && changed[i - 1] { emit[i - 1] = true; }
                    if i + 1 < num_cols && changed[i + 1] { emit[i + 1] = true; }
                }
            }

            for (i, info) in infos.iter().enumerate() {
                if i < num_cols && emit[i] {
                    glyphs[i] = Some(ShapedGlyph {
                        glyph_id: info.glyph_id,
                        cell_span: 1,
                        is_ligature: is_lig[i],
                    });
                }
            }
        } else {
            // Merged ligatures (fewer glyphs than chars): walk by cluster.
            let mut i = 0;
            while i < infos.len() {
                let cluster = infos[i].cluster as usize;
                let col = match byte_to_col.get(cluster) {
                    Some(&c) => c,
                    None => { i += 1; continue; },
                };

                // Find the end of this cluster.
                let mut j = i + 1;
                while j < infos.len() && infos[j].cluster == infos[i].cluster {
                    j += 1;
                }

                // Determine the column span from the next cluster's position.
                let next_col = if j < infos.len() {
                    byte_to_col.get(infos[j].cluster as usize).copied().unwrap_or(num_cols)
                } else {
                    num_cols
                };
                let cell_span = (next_col - col).clamp(1, 7) as u8;

                if col < num_cols {
                    glyphs[col] = Some(ShapedGlyph {
                        glyph_id: infos[i].glyph_id,
                        cell_span,
                        is_ligature: cell_span > 1,
                    });
                }

                i = j;
            }
        }

        let shaped = ShapedLine { glyphs };

        // Evict all entries when cache exceeds size limit to bound memory.
        if self.cache.len() >= MAX_SHAPE_CACHE_ENTRIES {
            self.cache.clear();
        }
        self.cache.insert(chars.to_vec(), shaped.clone());

        Some(shaped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const MONASPACE_NEON: &str =
        "/Users/pierloh/Library/Fonts/MonaspaceNeonNF-Regular.otf";

    fn load_font(path: &str) -> Option<Arc<[u8]>> {
        let p = Path::new(path);
        if !p.exists() { return None; }
        std::fs::read(p).ok().map(|v| Arc::from(v.into_boxed_slice()))
    }

    fn default_features() -> Vec<String> {
        ["calt", "liga", "ss01", "ss02", "ss03", "ss04", "ss05", "ss06", "ss07", "ss08", "ss09"]
            .iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn shaper_creates_from_valid_font() {
        let Some(data) = load_font(MONASPACE_NEON) else {
            eprintln!("SKIP: Monaspace font not installed");
            return;
        };
        assert!(Shaper::from_data(data, &default_features()).is_some());
    }

    #[test]
    fn shaper_returns_none_for_empty_features() {
        let Some(data) = load_font(MONASPACE_NEON) else {
            eprintln!("SKIP: Monaspace font not installed");
            return;
        };
        assert!(Shaper::from_data(data, &[]).is_none());
    }

    #[test]
    fn shape_line_detects_ligatures() {
        let Some(data) = load_font(MONASPACE_NEON) else {
            eprintln!("SKIP: Monaspace font not installed");
            return;
        };
        let mut shaper = Shaper::from_data(data, &default_features()).unwrap();
        let line: Vec<char> = "fn main() -> bool { x != y }".chars().collect();
        let result = shaper.shape_line(&line).unwrap();

        let shaped_count = result.glyphs.iter().filter(|g| g.is_some()).count();
        assert!(shaped_count > 0, "Should detect shaped glyphs in line with -> and !=");

        // Ligature members should be marked.
        let lig_count = result.glyphs.iter().filter(|g| {
            g.as_ref().map_or(false, |g| g.is_ligature)
        }).count();
        assert!(lig_count > 0, "Should have ligature-flagged glyphs for -> and !=");
    }

    #[test]
    fn shape_empty_line() {
        let Some(data) = load_font(MONASPACE_NEON) else {
            eprintln!("SKIP: Monaspace font not installed");
            return;
        };
        let mut shaper = Shaper::from_data(data, &default_features()).unwrap();
        let result = shaper.shape_line(&[]).unwrap();
        assert!(result.glyphs.is_empty());
    }

    #[test]
    fn cache_returns_consistent_results() {
        let Some(data) = load_font(MONASPACE_NEON) else {
            eprintln!("SKIP: Monaspace font not installed");
            return;
        };
        let mut shaper = Shaper::from_data(data, &default_features()).unwrap();
        let line: Vec<char> = "x -> y".chars().collect();

        let r1 = shaper.shape_line(&line).unwrap();
        let r2 = shaper.shape_line(&line).unwrap();

        assert_eq!(r1.glyphs.len(), r2.glyphs.len());
        for (a, b) in r1.glyphs.iter().zip(r2.glyphs.iter()) {
            match (a, b) {
                (Some(a), Some(b)) => {
                    assert_eq!(a.glyph_id, b.glyph_id);
                    assert_eq!(a.cell_span, b.cell_span);
                },
                (None, None) => {},
                _ => panic!("Cache returned inconsistent results"),
            }
        }
    }
}
