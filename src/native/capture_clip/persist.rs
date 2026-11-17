use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

use super::model::{ClipPlan, PlannedFileSource};

const STREAM_BUFFER_SIZE: usize = 64 * 1024;

pub(super) static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl ClipPlan {
    pub(crate) fn save(&self) -> Result<Vec<PathBuf>, String> {
        let mut created = Vec::new();
        let mut written = HashSet::new();

        for source in &self.attachment_source_checks {
            verify_attachment_source(
                &source.path,
                source.size,
                &source.sha256,
            )?;
        }

        for file in &self.files {
            if !written.insert(file.destination.clone()) {
                continue;
            }
            if file.reused {
                continue;
            }
            let result = match &file.source {
                PlannedFileSource::Inline(contents) => {
                    write_new_file_atomically(&file.destination, contents)
                }
                PlannedFileSource::Path(source) => write_source_atomically(
                    &file.destination,
                    source,
                    file.size,
                    &file.sha256,
                ),
            };
            if let Err(error) = result {
                let mut message = error;
                if !created.is_empty() {
                    let cleanup = cleanup_created(&created);
                    append_cleanup_message(&mut message, &cleanup);
                }
                return Err(message);
            }
            created.push(file.destination.clone());
        }

        Ok(created)
    }
}

pub(crate) fn cleanup_created(paths: &[PathBuf]) -> Vec<String> {
    let mut failures = Vec::new();
    for path in paths.iter().rev() {
        if let Err(error) = fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            failures.push(format!("remove {}: {error}", path.display()));
        }
    }
    failures
}

pub(crate) fn append_cleanup_message(
    message: &mut String,
    failures: &[String],
) {
    if failures.is_empty() {
        message.push_str("; removed clipboard files created by this capture");
    } else {
        message.push_str("; clipboard-file cleanup also failed: ");
        message.push_str(&failures.join("; "));
    }
}

fn write_new_file_atomically(
    destination: &Path,
    contents: &[u8],
) -> Result<(), String> {
    write_new_file_atomically_with(destination, contents, || Ok(()))
}

fn write_new_file_atomically_with(
    destination: &Path,
    contents: &[u8],
    before_install: impl FnOnce() -> io::Result<()>,
) -> Result<(), String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("clipboard-file");
    for _ in 0..100 {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{name}.bob-capture-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "create temporary clipboard file for {}: {error}",
                    destination.display()
                ));
            }
        };
        if let Err(error) =
            file.write_all(contents).and_then(|_| file.sync_all())
        {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "write temporary clipboard file for {}: {error}",
                destination.display()
            ));
        }
        drop(file);
        if let Err(error) = before_install() {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "prepare clipboard file {}: {error}",
                destination.display(),
            ));
        }
        let result = install_new_file(&temp, destination);
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        return result;
    }
    Err(format!(
        "could not allocate temporary clipboard file for {}",
        destination.display()
    ))
}

fn install_new_file(
    temporary: &Path,
    destination: &Path,
) -> Result<(), String> {
    match fs::hard_link(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(temporary);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(format!(
                "save clipboard file {}: destination appeared after planning",
                destination.display()
            ))
        }
        Err(error) if hard_link_unsupported(&error) => {
            let result = install_new_file_create_new(destination, temporary);
            let _ = fs::remove_file(temporary);
            result
        }
        Err(error) => Err(format!(
            "save clipboard file {}: {error}",
            destination.display()
        )),
    }
}

fn install_new_file_create_new(
    destination: &Path,
    source: &Path,
) -> Result<(), String> {
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "save clipboard file {}: destination appeared after planning",
                destination.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "save clipboard file {}: {error}",
                destination.display()
            ));
        }
    };
    let mut source_file = match fs::File::open(source) {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(destination);
            return Err(format!(
                "read temporary clipboard file for {}: {error}",
                destination.display()
            ));
        }
    };
    if let Err(error) =
        io::copy(&mut source_file, &mut file).and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "write clipboard file {}: {error}",
            destination.display()
        ));
    }
    Ok(())
}

fn write_source_atomically(
    destination: &Path,
    source: &Path,
    expected_size: u64,
    expected_hash: &str,
) -> Result<(), String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("clipboard-file");
    for _ in 0..100 {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{name}.bob-capture-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut destination_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "create temporary clipboard file for {}: {error}",
                    destination.display()
                ));
            }
        };
        let mut source_file = match fs::File::open(source) {
            Ok(file) => file,
            Err(_) => {
                let _ = fs::remove_file(&temp);
                return Err(source_changed_error(source));
            }
        };
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; STREAM_BUFFER_SIZE];
        let mut size = 0u64;
        loop {
            let read = match source_file.read(&mut buffer) {
                Ok(read) => read,
                Err(_) => {
                    let _ = fs::remove_file(&temp);
                    return Err(source_changed_error(source));
                }
            };
            if read == 0 {
                break;
            }
            if let Err(error) = destination_file.write_all(&buffer[..read]) {
                let _ = fs::remove_file(&temp);
                return Err(format!(
                    "write temporary clipboard file for {}: {error}",
                    destination.display()
                ));
            }
            hasher.update(&buffer[..read]);
            size = size.saturating_add(read as u64);
        }
        if let Err(error) = destination_file.sync_all() {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "write temporary clipboard file for {}: {error}",
                destination.display()
            ));
        }
        let hash = hex::encode(hasher.finalize());
        if size != expected_size || hash != expected_hash {
            let _ = fs::remove_file(&temp);
            return Err(source_changed_error(source));
        }
        drop(destination_file);
        let result = install_new_file(&temp, destination);
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        return result;
    }
    Err(format!(
        "could not allocate temporary clipboard file for {}",
        destination.display()
    ))
}

fn source_changed_error(source: &Path) -> String {
    format!(
        "clipboard attachment changed after planning: {}",
        source.display()
    )
}

fn verify_attachment_source(
    source: &Path,
    expected_size: u64,
    expected_hash: &str,
) -> Result<(), String> {
    match super::files::hash_file(source) {
        Ok((size, hash)) if size == expected_size && hash == expected_hash => {
            Ok(())
        }
        _ => Err(source_changed_error(source)),
    }
}

fn hard_link_unsupported(error: &io::Error) -> bool {
    match error.kind() {
        io::ErrorKind::Unsupported
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::CrossesDevices => true,
        _ => matches!(
            error.raw_os_error(),
            Some(libc::EPERM | libc::EXDEV | libc::ENOTSUP)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bob-clip-atomic-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temporary directory");
        path
    }

    #[test]
    fn attachment_install_refuses_a_destination_planted_after_planning() {
        let dir = temp_dir();
        let destination = dir.join("photo.png");
        let error = write_new_file_atomically_with(
            &destination,
            b"capture bytes",
            || fs::write(&destination, b"concurrent bytes"),
        )
        .expect_err("a planted destination must not be replaced");
        assert!(error.contains("destination appeared after planning"));
        assert_eq!(
            fs::read(destination).expect("planted file"),
            b"concurrent bytes"
        );
        assert_eq!(
            fs::read_dir(dir).expect("temp directory").count(),
            1,
            "temporary file is removed after the refused install"
        );
    }

    #[test]
    fn source_change_after_planning_aborts_without_installing_files() {
        let dir = temp_dir();
        let source = dir.join("source.png");
        let destination = dir.join("img/photo.png");
        fs::write(&source, b"planned bytes").expect("source");
        let planned_hash = hex::encode(Sha256::digest(b"planned bytes"));
        fs::write(&source, b"changed bytes").expect("mutate source");

        let error =
            write_source_atomically(&destination, &source, 13, &planned_hash)
                .expect_err("changed source must refuse the copy");
        assert!(error.contains("clipboard attachment changed after planning"));
        assert!(!destination.exists());
        assert_eq!(
            fs::read_dir(dir.join("img")).map_or(0, |rows| rows.count()),
            0
        );
    }

    #[test]
    fn create_new_fallback_copies_from_the_temporary_file() {
        let dir = temp_dir();
        let source = dir.join("temporary");
        let destination = dir.join("destination");
        fs::write(&source, b"streamed temp bytes").expect("temporary source");

        install_new_file_create_new(&destination, &source)
            .expect("fallback install");
        assert_eq!(
            fs::read(destination).expect("installed bytes"),
            b"streamed temp bytes"
        );
    }
}
