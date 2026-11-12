use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::model::ClipPlan;

pub(super) static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl ClipPlan {
    pub(crate) fn save(&self) -> Result<Vec<PathBuf>, String> {
        let mut created = Vec::new();
        let mut written = HashSet::new();

        for file in &self.files {
            if file.reused || !written.insert(file.destination.clone()) {
                continue;
            }
            if let Err(error) =
                write_new_file_atomically(&file.destination, &file.contents)
            {
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
        let result = install_new_file(&temp, destination, contents);
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
    contents: &[u8],
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
            let result = install_new_file_create_new(destination, contents);
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
    contents: &[u8],
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
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "write clipboard file {}: {error}",
            destination.display()
        ));
    }
    Ok(())
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
}
