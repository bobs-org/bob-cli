//! Path metadata, hashing, and atomic writes.
use super::*;

pub(super) fn ref_note_path(config: &Config, pdf: &Path) -> Result<PathBuf> {
    Ok(config
        .ref_dir
        .join(pdf_path_metadata(config, pdf)?.note_relative_path))
}

pub(super) fn pdf_path_metadata(
    config: &Config,
    pdf: &Path,
) -> Result<PdfPathMetadata> {
    let stem = pdf.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        CommandError::new(format!(
            "PDF path has no UTF-8 basename: {}",
            pdf.display()
        ))
    })?;

    if let Ok(relative_pdf_path) = pdf.strip_prefix(&config.lib_dir) {
        let mut note_relative_path = relative_pdf_path.to_path_buf();
        note_relative_path.set_extension("md");
        let ref_type = ref_type_from_relative_pdf_path(pdf, relative_pdf_path)?;
        return Ok(PdfPathMetadata {
            relative_pdf_path: Some(relative_pdf_path.to_path_buf()),
            note_relative_path,
            ref_type,
        });
    }
    if let Ok(relative_pdf_path) = pdf.strip_prefix(&config.xlib_dir) {
        let mut note_relative_path = relative_pdf_path.to_path_buf();
        note_relative_path.set_extension("md");
        let ref_type = ref_type_from_relative_pdf_path(pdf, relative_pdf_path)?;
        return Ok(PdfPathMetadata {
            relative_pdf_path: Some(relative_pdf_path.to_path_buf()),
            note_relative_path,
            ref_type,
        });
    }

    Ok(PdfPathMetadata {
        relative_pdf_path: None,
        note_relative_path: PathBuf::from(format!("{stem}.md")),
        ref_type: None,
    })
}

pub(super) fn ref_type_from_relative_pdf_path(
    pdf: &Path,
    relative_pdf_path: &Path,
) -> Result<Option<String>> {
    let Some(parent) = relative_pdf_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    else {
        return Ok(None);
    };
    let Some(component) = parent.components().find_map(|component| {
        if let Component::Normal(value) = component {
            Some(value)
        } else {
            None
        }
    }) else {
        return Ok(None);
    };
    let ref_type = component.to_str().ok_or_else(|| {
        CommandError::new(format!(
            "PDF path has non-UTF-8 ref_type component: {}",
            pdf.display()
        ))
    })?;
    Ok(Some(ref_type.to_string()))
}

pub(super) fn source_pdf_value(config: &Config, pdf: &Path) -> String {
    vault_relative_path_value(config, pdf)
}

pub(super) fn vault_relative_path_value(
    config: &Config,
    path: &Path,
) -> String {
    path.strip_prefix(&config.bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|error| {
        CommandError::new(format!(
            "read {} for sha256: {error}",
            path.display()
        ))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(super) fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let temp_path = temporary_write_path(path)?;
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, contents).map_err(|error| {
        CommandError::new(format!(
            "write temporary file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        CommandError::new(format!("install file {}: {error}", path.display()))
    })
}

pub(super) fn atomic_copy(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let temp_path = temporary_write_path(dest)?;
    let _ = fs::remove_file(&temp_path);
    fs::copy(source, &temp_path).map_err(|error| {
        CommandError::new(format!(
            "copy image asset {} to temporary file {}: {error}",
            source.display(),
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, dest).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        CommandError::new(format!(
            "install image asset {}: {error}",
            dest.display()
        ))
    })?;
    Ok(())
}

pub(super) fn temporary_write_path(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        CommandError::new(format!("path has no file name: {}", path.display()))
    })?;

    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", process::id()));
    Ok(path.with_file_name(temp_name))
}

pub(super) fn render_marker_scalar_string(value: &str) -> String {
    if can_render_plain_marker_scalar_string(value) {
        value.to_string()
    } else {
        quote_string(value)
    }
}

pub(super) fn render_marker_list_string(value: &str) -> String {
    if can_render_plain_marker_list_string(value) {
        value.to_string()
    } else {
        quote_string(value)
    }
}

pub(super) fn render_frontmatter_string(value: &str) -> String {
    if can_render_plain_frontmatter_string(value) {
        value.to_string()
    } else {
        quote_string(value)
    }
}

pub(super) fn can_render_plain_marker_scalar_string(value: &str) -> bool {
    if value.starts_with("[[") && value.ends_with("]]") {
        return true;
    }
    !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|character| matches!(character, '\n' | '\r'))
        && !is_marker_typed_literal(value)
        && !value.starts_with(['[', '{', '"', '\'', '-', '*', '#', '!', '&'])
}

pub(super) fn can_render_plain_marker_list_string(value: &str) -> bool {
    can_render_plain_marker_scalar_string(value)
        && !value.contains(',')
        && (is_wikilink(value)
            || !value
                .chars()
                .any(|character| matches!(character, '[' | ']')))
}

pub(super) fn can_render_plain_frontmatter_string(value: &str) -> bool {
    can_render_plain_marker_scalar_string(value)
        && !value.chars().any(char::is_whitespace)
        && !value.contains(':')
        && !value.contains('[')
        && !value.contains(']')
}

pub(super) fn is_marker_typed_literal(value: &str) -> bool {
    matches!(
        value,
        "null" | "true" | "false" | "~" | "Null" | "True" | "False"
    ) || is_number_literal(value)
}

pub(super) fn quote_string(value: &str) -> String {
    serde_json::to_string(value)
        .expect("serializing string to JSON cannot fail")
}

pub(super) fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}

pub(super) fn change_action(
    existed: bool,
    previous: Option<&str>,
    rendered: &str,
) -> &'static str {
    if previous == Some(rendered) {
        "none"
    } else if existed {
        "update"
    } else {
        "create"
    }
}
