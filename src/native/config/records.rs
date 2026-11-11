use std::path::Path;

use crate::native::records::schema::{
    parse_records_config, RecordDiagnostic, RecordDiagnosticCode, RecordsConfig,
};

pub(crate) fn load_records_config() -> Result<RecordsConfig, RecordDiagnostic> {
    load_records_config_from(&super::config_path())
}

pub(crate) fn load_records_config_from(
    path: &Path,
) -> Result<RecordsConfig, RecordDiagnostic> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecordsConfig::default());
        }
        Err(error) => {
            return Err(RecordDiagnostic::config(
                path,
                None,
                None,
                RecordDiagnosticCode::ConfigRead,
                format!("could not read config: {error}"),
            ));
        }
    };
    parse_records_config(&text, path)
}

#[cfg(test)]
mod tests {
    use super::load_records_config_from;
    use crate::native::config::plan::parse_plan_config;
    use crate::native::records::schema::parse_records_config;
    use std::{
        path::{Path, PathBuf},
        process,
    };

    #[test]
    fn missing_config_is_an_empty_records_config() {
        let config = load_records_config_from(Path::new(
            "/path/that/does/not/exist/records-config.yml",
        ))
        .expect("missing config is allowed");
        assert!(config.types.is_empty());
    }

    #[test]
    fn malformed_records_do_not_break_shared_plan_config_loading() {
        let text = "records: [not, a, mapping]\nplan: {}\n";
        assert!(parse_records_config(text, Path::new("/config.yml")).is_err());
        assert!(parse_plan_config(text, Path::new("/config.yml")).is_ok());
    }

    #[test]
    fn malformed_records_do_not_break_ordinary_capture_config_loading() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("records-config-{}.yml", process::id()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "records: [not, a, mapping]\ncapture:\n  max_saved_templates: 3\n",
        )
        .unwrap();
        let capture =
            crate::native::config::capture::load_capture_config(&path).unwrap();
        assert_eq!(capture.max_saved_templates(), 3);
        std::fs::remove_file(path).unwrap();
    }
}
