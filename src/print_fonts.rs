//! Pinned static print faces shared by note snapshots and the web-clip reader.
//!
//! Each file is `include_bytes!`'d once inside `bob-obsidian-assets` so the
//! Bob binary holds a single copy. Note snapshots write the full set into
//! the XeLaTeX scratch directory; the web-clip installer copies the reader
//! subset next to the adapter.

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrintFont {
    name: &'static str,
    source_path: &'static str,
    bytes: &'static [u8],
}

pub(crate) const SOURCE_SERIF4_SM_TEXT_REGULAR: &[u8] =
    bob_obsidian_assets::SOURCE_SERIF4_SM_TEXT_REGULAR;
pub(crate) const SOURCE_SERIF4_SM_TEXT_IT: &[u8] =
    bob_obsidian_assets::SOURCE_SERIF4_SM_TEXT_IT;
pub(crate) const SOURCE_SERIF4_SM_TEXT_SEMIBOLD: &[u8] =
    bob_obsidian_assets::SOURCE_SERIF4_SM_TEXT_SEMIBOLD;
pub(crate) const SOURCE_SERIF4_SM_TEXT_SEMIBOLD_IT: &[u8] =
    bob_obsidian_assets::SOURCE_SERIF4_SM_TEXT_SEMIBOLD_IT;
pub(crate) const INTER_REGULAR: &[u8] = bob_obsidian_assets::INTER_REGULAR;
pub(crate) const INTER_ITALIC: &[u8] = bob_obsidian_assets::INTER_ITALIC;
pub(crate) const INTER_MEDIUM: &[u8] = bob_obsidian_assets::INTER_MEDIUM;
pub(crate) const INTER_SEMIBOLD: &[u8] = bob_obsidian_assets::INTER_SEMIBOLD;
pub(crate) const INTER_SEMIBOLD_ITALIC: &[u8] =
    bob_obsidian_assets::INTER_SEMIBOLD_ITALIC;
pub(crate) const INTER_DISPLAY_SEMIBOLD: &[u8] =
    bob_obsidian_assets::INTER_DISPLAY_SEMIBOLD;
pub(crate) const JETBRAINS_MONO_REGULAR: &[u8] =
    bob_obsidian_assets::JETBRAINS_MONO_REGULAR;
pub(crate) const JETBRAINS_MONO_BOLD: &[u8] =
    bob_obsidian_assets::JETBRAINS_MONO_BOLD;
pub(crate) const NOTO_EMOJI_REGULAR: &[u8] =
    bob_obsidian_assets::NOTO_EMOJI_REGULAR;

#[cfg(test)]
const PRINT_FONTS: &[PrintFont] = &[
    PrintFont {
        name: "SourceSerif4SmText-Regular.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-Regular.otf",
        bytes: SOURCE_SERIF4_SM_TEXT_REGULAR,
    },
    PrintFont {
        name: "SourceSerif4SmText-It.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-It.otf",
        bytes: SOURCE_SERIF4_SM_TEXT_IT,
    },
    PrintFont {
        name: "SourceSerif4SmText-Semibold.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-Semibold.otf",
        bytes: SOURCE_SERIF4_SM_TEXT_SEMIBOLD,
    },
    PrintFont {
        name: "SourceSerif4SmText-SemiboldIt.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-SemiboldIt.otf",
        bytes: SOURCE_SERIF4_SM_TEXT_SEMIBOLD_IT,
    },
    PrintFont {
        name: "Inter-Regular.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-Regular.otf",
        bytes: INTER_REGULAR,
    },
    PrintFont {
        name: "Inter-Italic.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-Italic.otf",
        bytes: INTER_ITALIC,
    },
    PrintFont {
        name: "Inter-Medium.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-Medium.otf",
        bytes: INTER_MEDIUM,
    },
    PrintFont {
        name: "Inter-SemiBold.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-SemiBold.otf",
        bytes: INTER_SEMIBOLD,
    },
    PrintFont {
        name: "Inter-SemiBoldItalic.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-SemiBoldItalic.otf",
        bytes: INTER_SEMIBOLD_ITALIC,
    },
    PrintFont {
        name: "InterDisplay-SemiBold.otf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/InterDisplay-SemiBold.otf",
        bytes: INTER_DISPLAY_SEMIBOLD,
    },
    PrintFont {
        name: "JetBrainsMono-Regular.ttf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/JetBrainsMono-Regular.ttf",
        bytes: JETBRAINS_MONO_REGULAR,
    },
    PrintFont {
        name: "JetBrainsMono-Bold.ttf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/JetBrainsMono-Bold.ttf",
        bytes: JETBRAINS_MONO_BOLD,
    },
    PrintFont {
        name: "NotoEmoji-Regular.ttf",
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/NotoEmoji-Regular.ttf",
        bytes: NOTO_EMOJI_REGULAR,
    },
];

#[cfg(test)]
mod tests {
    use super::PRINT_FONTS;

    const WEB_CLIP_READER_NAMES: &[&str] = &[
        "SourceSerif4SmText-Regular.otf",
        "SourceSerif4SmText-It.otf",
        "SourceSerif4SmText-Semibold.otf",
        "SourceSerif4SmText-SemiboldIt.otf",
        "Inter-Regular.otf",
        "Inter-Italic.otf",
        "Inter-Medium.otf",
        "Inter-SemiBold.otf",
        "Inter-SemiBoldItalic.otf",
        "JetBrainsMono-Regular.ttf",
        "JetBrainsMono-Bold.ttf",
    ];

    #[test]
    fn web_clip_reader_reuses_print_font_bytes() {
        for name in WEB_CLIP_READER_NAMES {
            let font = PRINT_FONTS
                .iter()
                .find(|font| font.name == *name)
                .unwrap_or_else(|| panic!("missing print font {name}"));
            let install_path = format!("web_clip/fonts/{name}");
            let asset = crate::scripts::SUPPORT_ASSETS
                .iter()
                .find(|asset| asset.install_path == install_path)
                .unwrap_or_else(|| panic!("web-clip installer missing {name}"));
            assert_eq!(asset.source_path, font.source_path);
            assert!(
                std::ptr::eq(asset.contents, font.bytes),
                "{name} must reuse the snapshot print-font bytes"
            );
        }
    }

    #[test]
    fn web_clip_reader_omits_variable_woff2() {
        for asset in crate::scripts::SUPPORT_ASSETS {
            assert!(
                !asset.install_path.ends_with(".woff2"),
                "variable woff2 leftover: {}",
                asset.install_path
            );
        }
    }

    #[test]
    fn web_clip_reader_skips_display_and_emoji() {
        assert!(!WEB_CLIP_READER_NAMES.contains(&"InterDisplay-SemiBold.otf"));
        assert!(!WEB_CLIP_READER_NAMES.contains(&"NotoEmoji-Regular.ttf"));
        assert_eq!(WEB_CLIP_READER_NAMES.len(), 11);
        assert_eq!(PRINT_FONTS.len(), 13);
    }
}
