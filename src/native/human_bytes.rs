//! Shared one-decimal binary byte formatter (`B`/`KiB`/`MiB`).

/// Format a byte count with one decimal for `KiB`/`MiB` and no decimal
/// for bytes, e.g. `242.8 KiB`.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_kibibytes_and_mebibytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(248_678), "242.8 KiB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MiB");
    }
}
