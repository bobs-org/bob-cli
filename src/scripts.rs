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
        source_path: "scripts/web_clip/vendor/defuddle.full.js",
        install_path: "web_clip/vendor/defuddle.full.js",
        contents: include_bytes!("../scripts/web_clip/vendor/defuddle.full.js"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/vendor/DEFUDDLE_LICENSE",
        install_path: "web_clip/vendor/DEFUDDLE_LICENSE",
        contents: include_bytes!("../scripts/web_clip/vendor/DEFUDDLE_LICENSE"),
        executable: false,
    },
    EmbeddedAsset {
        source_path: "scripts/web_clip/vendor/README.md",
        install_path: "web_clip/vendor/README.md",
        contents: include_bytes!("../scripts/web_clip/vendor/README.md"),
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
