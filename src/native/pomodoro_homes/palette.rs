use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Hue {
    Blue,
    Orange,
    Aqua,
    Yellow,
    Magenta,
    Green,
    Violet,
    Red,
}

impl Hue {
    pub(crate) const ALL: [Self; 8] = [
        Self::Blue,
        Self::Orange,
        Self::Aqua,
        Self::Yellow,
        Self::Magenta,
        Self::Green,
        Self::Violet,
        Self::Red,
    ];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "blue" => Some(Self::Blue),
            "orange" => Some(Self::Orange),
            "aqua" => Some(Self::Aqua),
            "yellow" => Some(Self::Yellow),
            "magenta" => Some(Self::Magenta),
            "green" => Some(Self::Green),
            "violet" => Some(Self::Violet),
            "red" => Some(Self::Red),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Orange => "orange",
            Self::Aqua => "aqua",
            Self::Yellow => "yellow",
            Self::Magenta => "magenta",
            Self::Green => "green",
            Self::Violet => "violet",
            Self::Red => "red",
        }
    }

    pub(crate) const fn xterm(self) -> u8 {
        match self {
            Self::Blue => 33,
            Self::Orange => 208,
            Self::Aqua => 36,
            Self::Yellow => 214,
            Self::Magenta => 211,
            Self::Green => 28,
            Self::Violet => 62,
            Self::Red => 167,
        }
    }
}

pub(crate) fn fallback_hue(stem: &str) -> Hue {
    let hash = fnv1a32(stem.to_lowercase().as_bytes());
    Hue::ALL[(hash % Hue::ALL.len() as u32) as usize]
}

pub(crate) fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}
