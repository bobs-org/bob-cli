//! Embedded font and vendor payloads for Bob.
//!
//! This crate owns the byte-identical payloads Bob embeds: the print fonts in
//! `assets/print_fonts/`, the web-clip reader fonts in
//! `scripts/web_clip/fonts/`, and the vendored web-clip bundles in
//! `scripts/web_clip/vendor/`. Each file is `include_bytes!`'d once here so
//! the `bob-obsidian` binary holds a single copy through this crate.
//!
//! The crate is versioned independently from the root package and ships its
//! own notices. Consumers must keep `source_path` metadata pointed at this
//! crate's repository locations.

pub const SOURCE_SERIF4_SM_TEXT_REGULAR: &[u8] =
    include_bytes!("../assets/print_fonts/SourceSerif4SmText-Regular.otf");
pub const SOURCE_SERIF4_SM_TEXT_IT: &[u8] =
    include_bytes!("../assets/print_fonts/SourceSerif4SmText-It.otf");
pub const SOURCE_SERIF4_SM_TEXT_SEMIBOLD: &[u8] =
    include_bytes!("../assets/print_fonts/SourceSerif4SmText-Semibold.otf");
pub const SOURCE_SERIF4_SM_TEXT_SEMIBOLD_IT: &[u8] =
    include_bytes!("../assets/print_fonts/SourceSerif4SmText-SemiboldIt.otf");
pub const INTER_REGULAR: &[u8] =
    include_bytes!("../assets/print_fonts/Inter-Regular.otf");
pub const INTER_ITALIC: &[u8] =
    include_bytes!("../assets/print_fonts/Inter-Italic.otf");
pub const INTER_MEDIUM: &[u8] =
    include_bytes!("../assets/print_fonts/Inter-Medium.otf");
pub const INTER_SEMIBOLD: &[u8] =
    include_bytes!("../assets/print_fonts/Inter-SemiBold.otf");
pub const INTER_SEMIBOLD_ITALIC: &[u8] =
    include_bytes!("../assets/print_fonts/Inter-SemiBoldItalic.otf");
pub const INTER_DISPLAY_SEMIBOLD: &[u8] =
    include_bytes!("../assets/print_fonts/InterDisplay-SemiBold.otf");
pub const JETBRAINS_MONO_REGULAR: &[u8] =
    include_bytes!("../assets/print_fonts/JetBrainsMono-Regular.ttf");
pub const JETBRAINS_MONO_BOLD: &[u8] =
    include_bytes!("../assets/print_fonts/JetBrainsMono-Bold.ttf");
pub const NOTO_EMOJI_REGULAR: &[u8] =
    include_bytes!("../assets/print_fonts/NotoEmoji-Regular.ttf");
pub const PRINT_FONTS_OFL: &[u8] =
    include_bytes!("../assets/print_fonts/OFL.txt");
pub const PRINT_FONTS_README: &[u8] =
    include_bytes!("../assets/print_fonts/README.md");

pub const DEJAVU_SANS_TTF: &[u8] =
    include_bytes!("../scripts/web_clip/fonts/DejaVuSans.ttf");
pub const DEJAVU_LICENSE_TEXT: &[u8] =
    include_bytes!("../scripts/web_clip/fonts/DEJAVU_LICENSE.txt");
pub const READER_FONTS_OFL: &[u8] =
    include_bytes!("../scripts/web_clip/fonts/OFL.txt");
pub const READER_FONTS_README: &[u8] =
    include_bytes!("../scripts/web_clip/fonts/README.md");

pub const DEFUDDLE_JS: &[u8] =
    include_bytes!("../scripts/web_clip/vendor/defuddle.full.js");
pub const DEFUDDLE_LICENSE_TEXT: &[u8] =
    include_bytes!("../scripts/web_clip/vendor/DEFUDDLE_LICENSE");
pub const MERMAID_JS: &[u8] =
    include_bytes!("../scripts/web_clip/vendor/mermaid.min.js");
pub const MERMAID_LICENSE_TEXT: &[u8] =
    include_bytes!("../scripts/web_clip/vendor/MERMAID_LICENSE");
pub const VENDOR_README: &[u8] =
    include_bytes!("../scripts/web_clip/vendor/README.md");
