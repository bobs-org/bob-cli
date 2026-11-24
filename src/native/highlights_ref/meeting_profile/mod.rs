//! Meeting dossier print profile assets.
//!
//! The TeX header and Lua contract filter sit beside the note-snapshot
//! profile and share its fonts, glyph table, xelatex engine, and warning
//! parsing through [`super::note_snapshot::profile::render_profile_pdf`].
//! [`crate::native::meetings::render_meeting_pdf`] is the callable seam.

/// Embedded TeX header (written as `meeting-profile.tex` in scratch).
pub(crate) const MEETING_HEADER: &str = include_str!("meeting.tex");
/// Embedded Lua contract filter (written as `meeting.lua` in scratch).
pub(crate) const MEETING_FILTER: &str = include_str!("meeting.lua");

#[cfg(test)]
pub(crate) fn missing_tex_package_files() -> Result<Vec<String>, String> {
    use std::collections::BTreeSet;
    use std::path::Path;

    let xelatex = crate::native::env::find_on_path("xelatex")
        .ok_or_else(|| "xelatex is required".to_string())?;
    let beside_xelatex = xelatex
        .parent()
        .map(|dir| dir.join("kpsewhich"))
        .filter(|candidate| crate::native::env::is_executable_file(candidate));
    let kpsewhich = beside_xelatex
        .or_else(|| crate::native::env::find_on_path("kpsewhich"))
        .ok_or_else(|| {
            "kpsewhich is required to check TeX packages".to_string()
        })?;
    let packages = super::render_tex::RENDER_TEX_PACKAGES;
    let files: Vec<&str> =
        packages.iter().map(|package| package.file).collect();
    let output = std::process::Command::new(kpsewhich)
        .args(&files)
        .output()
        .map_err(|error| format!("kpsewhich failed: {error}"))?;
    let found: BTreeSet<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            Path::new(line.trim())
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    Ok(packages
        .iter()
        .filter(|package| !found.contains(package.file))
        .map(|package| package.file.to_string())
        .collect())
}
