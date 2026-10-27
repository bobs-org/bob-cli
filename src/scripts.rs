#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    Bash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptAsset {
    pub command: &'static str,
    pub source_path: &'static str,
    pub install_path: &'static str,
    pub kind: ScriptKind,
    pub contents: &'static [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedAsset {
    pub source_path: &'static str,
    pub install_path: &'static str,
    pub contents: &'static [u8],
    pub executable: bool,
}

impl ScriptAsset {
    pub const fn embedded_asset(&self) -> EmbeddedAsset {
        EmbeddedAsset {
            source_path: self.source_path,
            install_path: self.install_path,
            contents: self.contents,
            executable: true,
        }
    }
}

pub const SCRIPT_ASSETS: &[ScriptAsset] = &[
    ScriptAsset {
        command: "bob_pomodoro",
        source_path: "scripts/bob_pomodoro",
        install_path: "bob_pomodoro",
        kind: ScriptKind::Bash,
        contents: include_bytes!("../scripts/bob_pomodoro"),
    },
    ScriptAsset {
        command: "bob_notify",
        source_path: "scripts/bob_notify",
        install_path: "bob_notify",
        kind: ScriptKind::Bash,
        contents: include_bytes!("../scripts/bob_notify"),
    },
    ScriptAsset {
        command: "tmux_bob_pomodoro",
        source_path: "scripts/tmux_bob_pomodoro",
        install_path: "tmux_bob_pomodoro",
        kind: ScriptKind::Bash,
        contents: include_bytes!("../scripts/tmux_bob_pomodoro"),
    },
];

pub const SUPPORT_ASSETS: &[EmbeddedAsset] = &[
    EmbeddedAsset {
        source_path: "scripts/lib/bob_shell.sh",
        install_path: "lib/bob_shell.sh",
        contents: include_bytes!("../scripts/lib/bob_shell.sh"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/gkeep_adapter.py",
        install_path: "gkeep/gkeep_adapter.py",
        contents: include_bytes!("../scripts/gkeep_adapter.py"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/web_clip_adapter.py",
        install_path: "web_clip/web_clip_adapter.py",
        contents: include_bytes!("../scripts/web_clip/web_clip_adapter.py"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/snapshot.js",
        install_path: "web_clip/snapshot.js",
        contents: include_bytes!("../scripts/web_clip/snapshot.js"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/web_clip_render.py",
        install_path: "web_clip/web_clip_render.py",
        contents: include_bytes!("../scripts/web_clip/web_clip_render.py"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/diagram_render.py",
        install_path: "web_clip/diagram_render.py",
        contents: include_bytes!("../scripts/web_clip/diagram_render.py"),
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/vendor/defuddle.full.js",
        install_path: "web_clip/vendor/defuddle.full.js",
        contents: bob_obsidian_assets::DEFUDDLE_JS,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/vendor/DEFUDDLE_LICENSE",
        install_path: "web_clip/vendor/DEFUDDLE_LICENSE",
        contents: bob_obsidian_assets::DEFUDDLE_LICENSE_TEXT,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/vendor/mermaid.min.js",
        install_path: "web_clip/vendor/mermaid.min.js",
        contents: bob_obsidian_assets::MERMAID_JS,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/vendor/MERMAID_LICENSE",
        install_path: "web_clip/vendor/MERMAID_LICENSE",
        contents: bob_obsidian_assets::MERMAID_LICENSE_TEXT,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/vendor/README.md",
        install_path: "web_clip/vendor/README.md",
        contents: bob_obsidian_assets::VENDOR_README,
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/template/reader.css",
        install_path: "web_clip/template/reader.css",
        contents: include_bytes!("../scripts/web_clip/template/reader.css"),
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-Regular.otf",
        install_path: "web_clip/fonts/SourceSerif4SmText-Regular.otf",
        contents: crate::print_fonts::SOURCE_SERIF4_SM_TEXT_REGULAR,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-It.otf",
        install_path: "web_clip/fonts/SourceSerif4SmText-It.otf",
        contents: crate::print_fonts::SOURCE_SERIF4_SM_TEXT_IT,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-Semibold.otf",
        install_path: "web_clip/fonts/SourceSerif4SmText-Semibold.otf",
        contents: crate::print_fonts::SOURCE_SERIF4_SM_TEXT_SEMIBOLD,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/SourceSerif4SmText-SemiboldIt.otf",
        install_path: "web_clip/fonts/SourceSerif4SmText-SemiboldIt.otf",
        contents: crate::print_fonts::SOURCE_SERIF4_SM_TEXT_SEMIBOLD_IT,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-Regular.otf",
        install_path: "web_clip/fonts/Inter-Regular.otf",
        contents: crate::print_fonts::INTER_REGULAR,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-Italic.otf",
        install_path: "web_clip/fonts/Inter-Italic.otf",
        contents: crate::print_fonts::INTER_ITALIC,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-Medium.otf",
        install_path: "web_clip/fonts/Inter-Medium.otf",
        contents: crate::print_fonts::INTER_MEDIUM,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-SemiBold.otf",
        install_path: "web_clip/fonts/Inter-SemiBold.otf",
        contents: crate::print_fonts::INTER_SEMIBOLD,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/Inter-SemiBoldItalic.otf",
        install_path: "web_clip/fonts/Inter-SemiBoldItalic.otf",
        contents: crate::print_fonts::INTER_SEMIBOLD_ITALIC,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/JetBrainsMono-Regular.ttf",
        install_path: "web_clip/fonts/JetBrainsMono-Regular.ttf",
        contents: crate::print_fonts::JETBRAINS_MONO_REGULAR,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/assets/print_fonts/JetBrainsMono-Bold.ttf",
        install_path: "web_clip/fonts/JetBrainsMono-Bold.ttf",
        contents: crate::print_fonts::JETBRAINS_MONO_BOLD,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/fonts/OFL.txt",
        install_path: "web_clip/fonts/OFL.txt",
        contents: bob_obsidian_assets::READER_FONTS_OFL,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/fonts/DejaVuSans.ttf",
        install_path: "web_clip/fonts/DejaVuSans.ttf",
        contents: bob_obsidian_assets::DEJAVU_SANS_TTF,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/fonts/DEJAVU_LICENSE.txt",
        install_path: "web_clip/fonts/DEJAVU_LICENSE.txt",
        contents: bob_obsidian_assets::DEJAVU_LICENSE_TEXT,
        executable: false,
    },
    EmbeddedAsset {
        source_path:
            "crates/bob-obsidian-assets/scripts/web_clip/fonts/README.md",
        install_path: "web_clip/fonts/README.md",
        contents: bob_obsidian_assets::READER_FONTS_README,
        executable: false,
    },
];

pub fn script_names() -> impl Iterator<Item = &'static str> {
    SCRIPT_ASSETS.iter().map(|asset| asset.command)
}

pub fn script_by_command(command: &str) -> Option<&'static ScriptAsset> {
    SCRIPT_ASSETS.iter().find(|asset| asset.command == command)
}

pub fn embedded_assets() -> impl Iterator<Item = EmbeddedAsset> {
    SCRIPT_ASSETS
        .iter()
        .map(ScriptAsset::embedded_asset)
        .chain(SUPPORT_ASSETS.iter().copied())
}
