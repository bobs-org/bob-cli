//! LaTeX packages the Markdown PDF render needs.
//!
//! `bob ref create` renders Markdown through pandoc with xelatex. The render
//! depends on a fixed set of LaTeX packages (bob's own `header-includes`
//! plus pandoc's LaTeX template under bob's flags). This module declares
//! that set, checks it from `bob ref doctor`, and explains render failures.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// One LaTeX file the render may load, and the TeX Live package that ships it.
pub(super) struct TexPackage {
    pub(super) file: &'static str,
    pub(super) tlmgr: &'static str,
}

/// Every `.sty` file the Markdown render can load.
///
/// File names were verified with `tlmgr search --file`.
pub(super) const RENDER_TEX_PACKAGES: &[TexPackage] = &[
    // Bob's own headers (`PANDOC_HEADER_INCLUDES` and `return_links.tex`).
    TexPackage {
        file: "fvextra.sty",
        tlmgr: "fvextra",
    },
    TexPackage {
        file: "lineno.sty",
        tlmgr: "lineno",
    },
    TexPackage {
        file: "upquote.sty",
        tlmgr: "upquote",
    },
    TexPackage {
        file: "needspace.sty",
        tlmgr: "needspace",
    },
    TexPackage {
        file: "tikz.sty",
        tlmgr: "pgf",
    },
    // Pandoc's LaTeX template under bob's flags (xelatex, `linestretch`,
    // `geometry`, `colorlinks`, highlighting).
    TexPackage {
        file: "amsmath.sty",
        tlmgr: "amsmath",
    },
    TexPackage {
        file: "amssymb.sty",
        tlmgr: "amsfonts",
    },
    TexPackage {
        file: "setspace.sty",
        tlmgr: "setspace",
    },
    TexPackage {
        file: "iftex.sty",
        tlmgr: "iftex",
    },
    TexPackage {
        file: "unicode-math.sty",
        tlmgr: "unicode-math",
    },
    TexPackage {
        file: "fontspec.sty",
        tlmgr: "fontspec",
    },
    TexPackage {
        file: "lmodern.sty",
        tlmgr: "lm",
    },
    TexPackage {
        file: "xcolor.sty",
        tlmgr: "xcolor",
    },
    TexPackage {
        file: "geometry.sty",
        tlmgr: "geometry",
    },
    TexPackage {
        file: "fancyvrb.sty",
        tlmgr: "fancyvrb",
    },
    TexPackage {
        file: "framed.sty",
        tlmgr: "framed",
    },
    TexPackage {
        file: "hyperref.sty",
        tlmgr: "hyperref",
    },
    TexPackage {
        file: "bookmark.sty",
        tlmgr: "bookmark",
    },
    // Content-dependent template packages.
    TexPackage {
        file: "longtable.sty",
        tlmgr: "tools",
    },
    TexPackage {
        file: "booktabs.sty",
        tlmgr: "booktabs",
    },
    TexPackage {
        file: "etoolbox.sty",
        tlmgr: "etoolbox",
    },
    TexPackage {
        file: "footnote.sty",
        tlmgr: "mdwtools",
    },
    TexPackage {
        file: "graphicx.sty",
        tlmgr: "graphics",
    },
    TexPackage {
        file: "soul.sty",
        tlmgr: "soul",
    },
];

/// Return the declared packages missing from `kpsewhich` stdout.
///
/// `kpsewhich a.sty b.sty …` prints one path per found file, so anything
/// without a matching base name is missing. Order follows the declared list.
fn missing_packages(kpsewhich_stdout: &str) -> Vec<&'static TexPackage> {
    let found: BTreeSet<&str> = kpsewhich_stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            Path::new(line).file_name().and_then(|name| name.to_str())
        })
        .collect();
    RENDER_TEX_PACKAGES
        .iter()
        .filter(|package| !found.contains(package.file))
        .collect()
}

/// The single `tlmgr install` line that provides every missing package.
///
/// TeX Live names are deduplicated, keeping first-seen (list) order.
fn install_command(missing: &[&TexPackage]) -> String {
    let mut seen = BTreeSet::new();
    let mut names = Vec::new();
    for package in missing {
        if seen.insert(package.tlmgr) {
            names.push(package.tlmgr);
        }
    }
    format!("tlmgr install {}", names.join(" "))
}

/// Append the warning-level `xelatex` and `latex_packages` doctor rows.
///
/// These rows never fail the command: Markdown rendering is optional per
/// host, like the existing `pandoc` row.
pub(super) fn append_tex_doctor_rows(warnings: &mut Vec<String>) {
    let Some(xelatex_path) = crate::native::env::find_on_path("xelatex") else {
        println!("xelatex: warn (command not found)");
        warnings.push(
            "xelatex command not found; Markdown PDF creation is unavailable"
                .to_string(),
        );
        println!("latex_packages: skipped (no xelatex)");
        return;
    };
    println!("xelatex: available ({})", xelatex_path.display());

    let beside_xelatex = xelatex_path
        .parent()
        .map(|dir| dir.join("kpsewhich"))
        .filter(|candidate| crate::native::env::is_executable_file(candidate));
    let kpsewhich = beside_xelatex
        .or_else(|| crate::native::env::find_on_path("kpsewhich"));
    let Some(kpsewhich_path) = kpsewhich else {
        println!("latex_packages: warn (kpsewhich not found)");
        warnings.push(
            "kpsewhich command not found; LaTeX package check is unavailable"
                .to_string(),
        );
        return;
    };

    let files: Vec<&str> = RENDER_TEX_PACKAGES
        .iter()
        .map(|package| package.file)
        .collect();
    let output = match Command::new(&kpsewhich_path).args(&files).output() {
        Ok(output) => output,
        Err(error) => {
            println!("latex_packages: warn (kpsewhich failed: {error})");
            warnings.push(format!(
                "kpsewhich failed ({error}); LaTeX package check is unavailable"
            ));
            return;
        }
    };
    // A non-zero exit when something is missing is expected.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let missing = missing_packages(&stdout);
    if missing.is_empty() {
        println!("latex_packages: ok ({} checked)", RENDER_TEX_PACKAGES.len());
        return;
    }
    let files: Vec<&str> = missing.iter().map(|package| package.file).collect();
    let command = install_command(&missing);
    println!("latex_packages: warn (missing {})", files.join(", "));
    warnings.push(format!(
        "LaTeX packages missing for Markdown PDFs: {}; install with: {command}",
        files.join(", ")
    ));
}

/// Actionable hint for a pandoc render failure, if one applies.
pub(super) fn render_failure_hint(detail: &str) -> Option<String> {
    if let Some(file) = missing_tex_file(detail) {
        if let Some(package) = RENDER_TEX_PACKAGES
            .iter()
            .find(|package| package.file == file)
        {
            return Some(format!(
                "missing LaTeX package {file}; install it with `tlmgr install {}`, then run `bob ref doctor` to check the rest",
                package.tlmgr
            ));
        }
        return Some(format!(
            "missing LaTeX file {file}; find its package with `tlmgr search --global --file /{file}`, then run `bob ref doctor`"
        ));
    }
    if detail.contains("xelatex not found") {
        return Some(
            "install a TeX distribution that provides xelatex (e.g. TinyTeX), then run `bob ref doctor`"
                .to_string(),
        );
    }
    None
}

/// The `.sty` file in pandoc's `! LaTeX Error: File ... not found` line.
fn missing_tex_file(detail: &str) -> Option<&str> {
    const MARKER: &str = "! LaTeX Error: File `";
    const SUFFIX: &str = "' not found";
    let start = detail.find(MARKER)? + MARKER.len();
    let rest = detail.get(start..)?;
    let end = rest.find(SUFFIX)?;
    let file = rest.get(..end)?;
    (!file.is_empty()).then_some(file)
}

#[cfg(test)]
mod tests {
    use super::super::create::PANDOC_HEADER_INCLUDES;
    use super::super::return_links::HEADER_INCLUDES;
    use super::*;

    /// Every `\usepackage[…]{a,b}` in bob's headers must be declared above.
    ///
    /// Adding a header package without declaring it fails here on every
    /// machine, instead of failing only at render time on a minimal TeX tree.
    #[test]
    fn header_packages_are_all_declared() {
        let mut names = BTreeSet::new();
        for header in [PANDOC_HEADER_INCLUDES, HEADER_INCLUDES] {
            names.extend(tex_packages_in(header));
        }
        let declared: BTreeSet<&str> = RENDER_TEX_PACKAGES
            .iter()
            .map(|package| {
                package
                    .file
                    .strip_suffix(".sty")
                    .expect("declared file ends in .sty")
            })
            .collect();
        let undeclared: Vec<&str> =
            names.difference(&declared).copied().collect();
        assert!(
            undeclared.is_empty(),
            "header packages missing from RENDER_TEX_PACKAGES: {}",
            undeclared.join(", ")
        );
    }

    /// `\usepackage` names in one header string, in first-seen order.
    fn tex_packages_in(header: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = header;
        while let Some(start) = rest.find("\\usepackage") {
            rest = &rest[start + "\\usepackage".len()..];
            // Optional `[options]` before the `{names}` group.
            if let Some(bracket) = rest.strip_prefix('[')
                && let Some(end) = bracket.find(']')
            {
                rest = &bracket[end + 1..];
            }
            let Some(open) = rest.find('{') else {
                break;
            };
            rest = &rest[open + 1..];
            let Some(close) = rest.find('}') else {
                break;
            };
            let (group, tail) = rest.split_at(close);
            rest = &tail[1..];
            for name in group
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out
    }

    #[test]
    fn missing_packages_returns_rest_in_order() {
        let stdout = "/tex/fvextra.sty\n/tex/lineno.sty\n";
        let missing = missing_packages(stdout);
        assert!(!missing.is_empty());
        assert!(
            !missing.iter().any(|package| package.file == "fvextra.sty"),
            "found files must not be reported missing"
        );
        assert!(
            !missing.iter().any(|package| package.file == "lineno.sty"),
            "found files must not be reported missing"
        );
        assert!(missing
            .iter()
            .any(|package| package.file == "needspace.sty"));
        // List order is preserved.
        let mut files: Vec<&str> =
            missing.iter().map(|package| package.file).collect();
        let mut ordered = files.clone();
        ordered.sort_by_key(|file| {
            RENDER_TEX_PACKAGES
                .iter()
                .position(|package| package.file == *file)
        });
        files.sort_by_key(|file| {
            RENDER_TEX_PACKAGES
                .iter()
                .position(|package| package.file == *file)
        });
        assert_eq!(files, ordered);
    }

    #[test]
    fn install_command_deduplicates_tlmgr_names() {
        let stdout = String::new();
        let mut missing = missing_packages(&stdout);
        // `amsymb.sty` maps to `amsfonts`, which no other entry uses, so
        // force a duplicate by repeating the first entry.
        if let Some(first) = missing.first() {
            let repeated = *first;
            missing.push(repeated);
        }
        let command = install_command(&missing);
        let names: Vec<&str> = command
            .strip_prefix("tlmgr install ")
            .expect("install command prefix")
            .split_whitespace()
            .collect();
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "tlmgr names must be deduplicated: {command}"
        );
        assert!(
            command.contains("needspace"),
            "install command must name the missing package: {command}"
        );
    }

    #[test]
    fn render_failure_hint_for_listed_package() {
        let detail = "Error producing PDF.\n! LaTeX Error: File `needspace.sty' not found.\nType X to quit.";
        assert_eq!(
            render_failure_hint(detail),
            Some(
                "missing LaTeX package needspace.sty; install it with `tlmgr install needspace`, then run `bob ref doctor` to check the rest"
                    .to_string()
            )
        );
    }

    #[test]
    fn render_failure_hint_for_unlisted_file() {
        let detail =
            "! LaTeX Error: File `obscure.sty' not found.\nSee the manual.";
        assert_eq!(
            render_failure_hint(detail),
            Some(
                "missing LaTeX file obscure.sty; find its package with `tlmgr search --global --file /obscure.sty`, then run `bob ref doctor`"
                    .to_string()
            )
        );
    }

    #[test]
    fn render_failure_hint_for_missing_xelatex() {
        let detail = "pandoc: xelatex not found. Please select a different --pdf-engine or install xelatex";
        assert_eq!(
            render_failure_hint(detail),
            Some(
                "install a TeX distribution that provides xelatex (e.g. TinyTeX), then run `bob ref doctor`"
                    .to_string()
            )
        );
    }

    #[test]
    fn render_failure_hint_for_unrelated_error() {
        assert_eq!(
            render_failure_hint("pandoc produced no diagnostic output"),
            None
        );
    }
}
