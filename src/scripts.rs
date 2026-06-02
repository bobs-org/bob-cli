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
        command: "bob_sync",
        source_path: "scripts/bob_sync",
        install_path: "bob_sync",
        kind: ScriptKind::Bash,
        contents: include_bytes!("../scripts/bob_sync"),
    },
    ScriptAsset {
        command: "tmux_bob_pomodoro",
        source_path: "scripts/tmux_bob_pomodoro",
        install_path: "tmux_bob_pomodoro",
        kind: ScriptKind::Bash,
        contents: include_bytes!("../scripts/tmux_bob_pomodoro"),
    },
];

pub const SUPPORT_ASSETS: &[EmbeddedAsset] = &[EmbeddedAsset {
    source_path: "scripts/lib/bob_shell.sh",
    install_path: "lib/bob_shell.sh",
    contents: include_bytes!("../scripts/lib/bob_shell.sh"),
    executable: false,
}];

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
