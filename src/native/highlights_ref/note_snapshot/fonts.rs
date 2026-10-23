use std::{
    collections::BTreeSet,
    fs,
    io::{self, BufWriter, Write},
    path::Path,
};

use ttf_parser::Face;

use crate::print_fonts;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FontRole {
    Serif,
    Sans,
    Mono,
    Fallback,
}

struct FontAsset {
    name: &'static str,
    role: FontRole,
    bytes: &'static [u8],
}

const FONT_ASSETS: &[FontAsset] = &[
    FontAsset {
        name: "SourceSerif4SmText-Regular.otf",
        role: FontRole::Serif,
        bytes: print_fonts::SOURCE_SERIF4_SM_TEXT_REGULAR,
    },
    FontAsset {
        name: "SourceSerif4SmText-It.otf",
        role: FontRole::Serif,
        bytes: print_fonts::SOURCE_SERIF4_SM_TEXT_IT,
    },
    FontAsset {
        name: "SourceSerif4SmText-Semibold.otf",
        role: FontRole::Serif,
        bytes: print_fonts::SOURCE_SERIF4_SM_TEXT_SEMIBOLD,
    },
    FontAsset {
        name: "SourceSerif4SmText-SemiboldIt.otf",
        role: FontRole::Serif,
        bytes: print_fonts::SOURCE_SERIF4_SM_TEXT_SEMIBOLD_IT,
    },
    FontAsset {
        name: "Inter-Regular.otf",
        role: FontRole::Sans,
        bytes: print_fonts::INTER_REGULAR,
    },
    FontAsset {
        name: "Inter-Italic.otf",
        role: FontRole::Sans,
        bytes: print_fonts::INTER_ITALIC,
    },
    FontAsset {
        name: "Inter-Medium.otf",
        role: FontRole::Sans,
        bytes: print_fonts::INTER_MEDIUM,
    },
    FontAsset {
        name: "Inter-SemiBold.otf",
        role: FontRole::Sans,
        bytes: print_fonts::INTER_SEMIBOLD,
    },
    FontAsset {
        name: "Inter-SemiBoldItalic.otf",
        role: FontRole::Sans,
        bytes: print_fonts::INTER_SEMIBOLD_ITALIC,
    },
    FontAsset {
        name: "InterDisplay-SemiBold.otf",
        role: FontRole::Sans,
        bytes: print_fonts::INTER_DISPLAY_SEMIBOLD,
    },
    FontAsset {
        name: "JetBrainsMono-Regular.ttf",
        role: FontRole::Mono,
        bytes: print_fonts::JETBRAINS_MONO_REGULAR,
    },
    FontAsset {
        name: "JetBrainsMono-Bold.ttf",
        role: FontRole::Mono,
        bytes: print_fonts::JETBRAINS_MONO_BOLD,
    },
    FontAsset {
        name: "NotoEmoji-Regular.ttf",
        role: FontRole::Fallback,
        bytes: print_fonts::NOTO_EMOJI_REGULAR,
    },
];

const VARIATION_SELECTOR_16: char = '\u{fe0f}';

struct ParsedFont {
    name: &'static str,
    role: FontRole,
    face: Face<'static>,
}

/// Cmap coverage for the four font roles used by the note print profile.
pub(crate) struct GlyphCoverage {
    fonts: Vec<ParsedFont>,
}

impl GlyphCoverage {
    /// Parses the bundled font cmaps once so repeated coverage checks stay cheap.
    pub(crate) fn new() -> io::Result<Self> {
        let fonts = FONT_ASSETS
            .iter()
            .map(|asset| {
                let face = Face::parse(asset.bytes, 0).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "could not parse bundled font {}: {error:?}",
                            asset.name
                        ),
                    )
                })?;
                Ok(ParsedFont {
                    name: asset.name,
                    role: asset.role,
                    face,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        Ok(Self { fonts })
    }

    /// Returns whether any bundled face for `role` has a glyph for `ch`.
    ///
    /// U+FE0F is intentionally considered uncovered: the note print profile strips
    /// this presentation selector before rendering.
    pub(crate) fn covers(&self, role: FontRole, ch: char) -> bool {
        if ch == VARIATION_SELECTOR_16 {
            return false;
        }

        self.fonts
            .iter()
            .filter(|font| font.role == role)
            .any(|font| font.face.glyph_index(ch).is_some())
    }

    /// Unique characters absent from all three body roles, in Unicode order.
    pub(crate) fn body_uncovered(&self, text: &str) -> Vec<char> {
        unique_chars(text)
            .into_iter()
            .filter(|ch| !self.body_covers(*ch))
            .collect()
    }

    /// Unique characters absent from every body and fallback face, in Unicode order.
    pub(crate) fn uncovered_everywhere(&self, text: &str) -> Vec<char> {
        unique_chars(text)
            .into_iter()
            .filter(|ch| !self.any_role_covers(*ch))
            .collect()
    }

    /// Writes all bundled faces into the scratch directory's `fonts/` directory.
    pub(crate) fn write_fonts(dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        for asset in FONT_ASSETS {
            fs::write(dir.join(asset.name), asset.bytes)?;
        }
        Ok(())
    }

    /// Writes a Lua integer-keyed set of glyphs that require the fallback face,
    /// plus per-role body-font overrides.
    ///
    /// The integer keys preserve the original contract: every codepoint covered
    /// by the fallback face but by no body face. The named `serif_uncovered`,
    /// `sans_uncovered`, and `mono_uncovered` tables list codepoints missing
    /// from that role but present in another bundled face (body or fallback),
    /// so the Lua filter can select another body face before Noto. In
    /// particular `▸` (U+25B8) lives only in JetBrains Mono: the union table
    /// alone would claim body coverage while serif/sans text still renders
    /// tofu, so the per-role tables direct serif/sans runs to the mono face.
    pub(crate) fn write_glyph_table(&self, path: &Path) -> io::Result<()> {
        let mut fallback_chars = BTreeSet::new();
        let mut all_chars = BTreeSet::new();

        for font in self.fonts.iter() {
            let Some(cmap) = font.face.tables().cmap else {
                continue;
            };

            for subtable in cmap.subtables {
                if !subtable.is_unicode() {
                    continue;
                }

                subtable.codepoints(|codepoint| {
                    let Some(ch) = char::from_u32(codepoint) else {
                        return;
                    };
                    if ch == VARIATION_SELECTOR_16 {
                        return;
                    }
                    if font.face.glyph_index(ch).is_none() {
                        return;
                    }
                    all_chars.insert(ch);
                    if font.role == FontRole::Fallback && !self.body_covers(ch)
                    {
                        fallback_chars.insert(ch);
                    }
                });
            }
        }

        let mut serif_uncovered = BTreeSet::new();
        let mut sans_uncovered = BTreeSet::new();
        let mut mono_uncovered = BTreeSet::new();
        for ch in &all_chars {
            if !self.covers(FontRole::Serif, *ch)
                && (self.covers(FontRole::Sans, *ch)
                    || self.covers(FontRole::Mono, *ch)
                    || self.covers(FontRole::Fallback, *ch))
            {
                serif_uncovered.insert(*ch);
            }
            if !self.covers(FontRole::Sans, *ch)
                && (self.covers(FontRole::Serif, *ch)
                    || self.covers(FontRole::Mono, *ch)
                    || self.covers(FontRole::Fallback, *ch))
            {
                sans_uncovered.insert(*ch);
            }
            if !self.covers(FontRole::Mono, *ch)
                && (self.covers(FontRole::Serif, *ch)
                    || self.covers(FontRole::Sans, *ch)
                    || self.covers(FontRole::Fallback, *ch))
            {
                mono_uncovered.insert(*ch);
            }
        }

        let mut writer = BufWriter::new(fs::File::create(path)?);
        writeln!(writer, "return {{")?;
        for ch in fallback_chars {
            writeln!(writer, "  [0x{:X}] = true,", u32::from(ch))?;
        }
        writeln!(writer, "  serif_uncovered = {{")?;
        for ch in serif_uncovered {
            writeln!(writer, "    [0x{:X}] = true,", u32::from(ch))?;
        }
        writeln!(writer, "  }},")?;
        writeln!(writer, "  sans_uncovered = {{")?;
        for ch in sans_uncovered {
            writeln!(writer, "    [0x{:X}] = true,", u32::from(ch))?;
        }
        writeln!(writer, "  }},")?;
        writeln!(writer, "  mono_uncovered = {{")?;
        for ch in mono_uncovered {
            writeln!(writer, "    [0x{:X}] = true,", u32::from(ch))?;
        }
        writeln!(writer, "  }},")?;
        writeln!(writer, "}}")
    }

    fn body_covers(&self, ch: char) -> bool {
        [FontRole::Serif, FontRole::Sans, FontRole::Mono]
            .into_iter()
            .any(|role| self.covers(role, ch))
    }

    fn any_role_covers(&self, ch: char) -> bool {
        [
            FontRole::Serif,
            FontRole::Sans,
            FontRole::Mono,
            FontRole::Fallback,
        ]
        .into_iter()
        .any(|role| self.covers(role, ch))
    }
}

fn unique_chars(text: &str) -> BTreeSet<char> {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !ch.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use sha2::{Digest, Sha256};

    use super::{FontRole, GlyphCoverage, FONT_ASSETS, VARIATION_SELECTOR_16};

    const CORPUS_CODEPOINTS: &str = "→ · — – • ” “ ’ … ≤ ▸ ¶ á ķ\u{00a0}\u{0326}\u{fe0f}🎲 🗓 ⚪ 🔵 🔴 🟢 ⛓ ✅ 💡 🤷 ❌ 🛠 🧱 🧩 🔖 🍅 🤯";

    #[test]
    fn bundled_font_hashes_match_the_release_manifest() {
        let expected = [
            ("SourceSerif4SmText-Regular.otf", "760327b0f9ef55310f11ed5c955693f4841f9b859407e98570d3654422a34d70"),
            ("SourceSerif4SmText-It.otf", "220a0bf6d2a0c3fde644db2b2b602e725170ac1a90b0ce79994b7598c12980d1"),
            ("SourceSerif4SmText-Semibold.otf", "1e32888469460231d1d43369a1cf88d2f8279571893418e653945171ac91bcb8"),
            ("SourceSerif4SmText-SemiboldIt.otf", "965a1732d7d1b4550d2db3a7fe8aea5c9245fc1e98bb88077d089acd995ac71a"),
            ("Inter-Regular.otf", "d4f2b9e148059a15f014cb0f0b8fea8cd11bfa447dd483bedf1b0adc0e2ba799"),
            ("Inter-Italic.otf", "3bdf49f72ad2f2959a2619ceef700784504f6d17f502e2e2bc6edce5d43ca631"),
            ("Inter-Medium.otf", "6c222ed2c3660524dd0c14fb745e00fee65c5361ab28f60fba6730bf4e648763"),
            ("Inter-SemiBold.otf", "0a4d30778fa2dc239368d90fd854b88b27e7ede737450da72686a90eca66f02c"),
            ("Inter-SemiBoldItalic.otf", "a350102650a86612efa66e0291315dba24be43e1f3716359c17d365376bea11e"),
            ("InterDisplay-SemiBold.otf", "991234562ac06b47aefa2ca4d4ff74360a164a5653ec05357816fc4ffe3ca8a2"),
            ("JetBrainsMono-Regular.ttf", "a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f"),
            ("JetBrainsMono-Bold.ttf", "5590990c82e097397517f275f430af4546e1c45cff408bde4255dad142479dcb"),
            ("NotoEmoji-Regular.ttf", "988621dc5c9a75eb6144f28faae30317a8e3421b68b28740747b3d739e2326b8"),
        ];

        assert_eq!(FONT_ASSETS.len(), expected.len());
        for (asset, (expected_name, expected_hash)) in
            FONT_ASSETS.iter().zip(expected)
        {
            assert_eq!(asset.name, expected_name);
            assert_eq!(hex::encode(Sha256::digest(asset.bytes)), expected_hash);
        }
    }

    #[test]
    fn bundled_faces_cover_the_snapshot_corpus() {
        let coverage =
            GlyphCoverage::new().expect("bundled fonts should parse");
        let missing = coverage
            .uncovered_everywhere(CORPUS_CODEPOINTS)
            .into_iter()
            .filter(|ch| *ch != VARIATION_SELECTOR_16)
            .collect::<Vec<_>>();

        assert!(missing.is_empty(), "uncovered corpus glyphs: {missing:?}");
        assert!(!coverage.covers(FontRole::Fallback, VARIATION_SELECTOR_16));
        for ch in CORPUS_CODEPOINTS
            .chars()
            .filter(|ch| *ch != VARIATION_SELECTOR_16)
        {
            assert!(
                coverage.body_covers(ch)
                    || coverage.covers(FontRole::Fallback, ch),
                "no body or fallback face covers {ch:?} (U+{:04X})",
                u32::from(ch)
            );
        }
    }

    #[test]
    fn coverage_reports_body_and_total_gaps_separately() {
        let coverage =
            GlyphCoverage::new().expect("bundled fonts should parse");
        assert_eq!(coverage.body_uncovered("A🎲"), vec!['🎲']);
        assert!(coverage.uncovered_everywhere("A🎲").is_empty());
        assert!(coverage.uncovered_everywhere("A\n\t\r").is_empty());
        assert_eq!(
            coverage.uncovered_everywhere("\u{fe0f}"),
            vec![VARIATION_SELECTOR_16]
        );
    }

    #[test]
    fn write_fonts_materializes_every_embedded_face() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let output = temp.path().join("fonts");
        GlyphCoverage::write_fonts(&output).expect("font files should write");

        for asset in FONT_ASSETS {
            assert_eq!(
                fs::read(output.join(asset.name))
                    .expect("written font should read"),
                asset.bytes,
                "wrong bytes written for {}",
                asset.name
            );
        }
    }

    #[test]
    fn glyph_table_lists_fallback_only_characters() {
        let coverage =
            GlyphCoverage::new().expect("bundled fonts should parse");
        let temp = tempfile::tempdir().expect("temporary directory");
        let table = temp.path().join("glyphs.lua");
        coverage
            .write_glyph_table(Path::new(&table))
            .expect("glyph table should write");
        let source =
            fs::read_to_string(table).expect("glyph table should be UTF-8");

        assert!(source.contains("[0x1F3B2] = true,"), "dice needs fallback");
        assert!(
            !source.contains("[0x41] = true,"),
            "Latin A is a body glyph"
        );
        assert!(
            !source.contains("[0xFE0F] = true,"),
            "presentation selector is stripped before rendering"
        );
    }

    #[test]
    fn triangular_bullet_selects_mono_before_fallback() {
        let coverage =
            GlyphCoverage::new().expect("bundled fonts should parse");
        assert!(
            coverage.covers(FontRole::Mono, '▸'),
            "JetBrains Mono must cover U+25B8"
        );
        assert!(
            !coverage.covers(FontRole::Serif, '▸'),
            "Source Serif must miss U+25B8 for this regression"
        );
        assert!(
            !coverage.covers(FontRole::Sans, '▸'),
            "Inter must miss U+25B8 for this regression"
        );
        assert!(
            !coverage.covers(FontRole::Fallback, '▸'),
            "Noto Emoji must miss U+25B8 for this regression"
        );

        let temp = tempfile::tempdir().expect("temporary directory");
        let table = temp.path().join("glyphs.lua");
        coverage
            .write_glyph_table(Path::new(&table))
            .expect("glyph table should write");
        let source =
            fs::read_to_string(table).expect("glyph table should be UTF-8");
        assert!(
            source.contains("serif_uncovered"),
            "glyph table must carry per-role overrides"
        );
        assert!(
            source.contains("[0x25B8] = true,"),
            "U+25B8 must appear as a per-role override, not a union hit"
        );
    }
}
