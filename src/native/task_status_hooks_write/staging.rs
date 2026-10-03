use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};

use super::model::{
    remaining_outputs, ApplyError, ApplySession, PlannedWrite, StagedWrite,
    WritePlan,
};

pub(super) fn stage_outputs(
    plan: &WritePlan,
    session: &ApplySession,
    recovery_dir: &Path,
) -> Result<Vec<StagedWrite>, ApplyError> {
    let mut staged = Vec::new();
    for (index, output) in plan.outputs.iter().enumerate() {
        match stage_one(output, &session.run_id, index) {
            Ok(item) => {
                if let Some(fail) = &session.fail_staging
                    && let Err(error) = fail(&output.path)
                {
                    staged.push(item);
                    cleanup_temps(&staged, session.tool);
                    return Err(ApplyError::io(
                        format!(
                            "failed to stage {}: {error}",
                            output.path.display()
                        ),
                        Vec::new(),
                        remaining_outputs(plan, &[]),
                        Some(recovery_dir.to_path_buf()),
                    ));
                }
                staged.push(item);
            }
            Err(error) => {
                cleanup_temps(&staged, session.tool);
                return Err(ApplyError::io(
                    format!(
                        "failed to stage {}: {error}",
                        output.path.display()
                    ),
                    Vec::new(),
                    remaining_outputs(plan, &[]),
                    Some(recovery_dir.to_path_buf()),
                ));
            }
        }
    }
    Ok(staged)
}

fn stage_one(
    output: &PlannedWrite,
    run_id: &str,
    index: usize,
) -> io::Result<StagedWrite> {
    let parent = output.path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = output.path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", output.path.display()),
        )
    })?;
    let mut nonce = 0_u32;
    let temp = loop {
        let temp_name = staged_temp_name(file_name, run_id, index, nonce);
        let temp = parent.join(temp_name);
        match create_exclusive_temp(
            &temp,
            &output.proposed_bytes,
            output.identity.mode,
        ) {
            Ok(()) => break temp,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                nonce = nonce.checked_add(1).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "exhausted exclusive temporary names",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    };
    if let Err(error) = copy_copied_metadata(&output.path, &temp) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(StagedWrite {
        dest: output.path.clone(),
        temp,
        proposed_bytes: output.proposed_bytes.clone(),
    })
}

fn staged_temp_name(
    file_name: &OsStr,
    run_id: &str,
    index: usize,
    nonce: u32,
) -> OsString {
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".bob-tsh.{run_id}.{index}.{nonce}.tmp"));
    name
}

fn create_exclusive_temp(
    path: &Path,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    let _ = mode;
    Ok(())
}

fn copy_copied_metadata(from: &Path, to: &Path) -> io::Result<()> {
    copy_xattrs(from, to)
}

pub(super) fn cleanup_temps(staged: &[StagedWrite], tool: &str) {
    for item in staged {
        if let Err(error) = fs::remove_file(&item.temp)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "bob {tool}: warning: failed to remove staging file {}: {error}",
                item.temp.display()
            );
        }
    }
}

pub(super) fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(target_os = "linux")]
fn copy_xattrs(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::{
        raw::{c_char, c_int},
        unix::ffi::OsStrExt,
    };

    unsafe extern "C" {
        fn llistxattr(
            path: *const c_char,
            list: *mut c_char,
            size: usize,
        ) -> isize;
        fn lgetxattr(
            path: *const c_char,
            name: *const c_char,
            value: *mut u8,
            size: usize,
        ) -> isize;
        fn lsetxattr(
            path: *const c_char,
            name: *const c_char,
            value: *const u8,
            size: usize,
            flags: c_int,
        ) -> c_int;
    }

    const ENOTSUP: i32 = 95;
    const ENOSYS: i32 = 38;

    fn cstring(path: &Path) -> io::Result<std::ffi::CString> {
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL")
        })
    }

    fn copyable(name: &str) -> bool {
        name.starts_with("user.")
            || name.starts_with("trusted.")
            || name == "system.posix_acl_access"
            || name == "system.posix_acl_default"
    }

    let from_c = cstring(from)?;
    let to_c = cstring(to)?;
    let size = unsafe { llistxattr(from_c.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(ENOTSUP | ENOSYS) => Ok(()),
            _ => Err(error),
        };
    }
    if size == 0 {
        return Ok(());
    }
    let mut list = vec![0_u8; size as usize];
    let written = unsafe {
        llistxattr(
            from_c.as_ptr(),
            list.as_mut_ptr() as *mut c_char,
            list.len(),
        )
    };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    list.truncate(written as usize);
    for name in list
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name_c = std::ffi::CString::new(name).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "xattr name contains NUL",
            )
        })?;
        let name_text = name_c.to_string_lossy();
        if !copyable(&name_text) {
            continue;
        }
        let value_size = unsafe {
            lgetxattr(from_c.as_ptr(), name_c.as_ptr(), std::ptr::null_mut(), 0)
        };
        if value_size < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = vec![0_u8; value_size as usize];
        let got = unsafe {
            lgetxattr(
                from_c.as_ptr(),
                name_c.as_ptr(),
                value.as_mut_ptr(),
                value.len(),
            )
        };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        value.truncate(got as usize);
        let set = unsafe {
            lsetxattr(
                to_c.as_ptr(),
                name_c.as_ptr(),
                value.as_ptr(),
                value.len(),
                0,
            )
        };
        if set < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn copy_xattrs(_from: &Path, _to: &Path) -> io::Result<()> {
    Ok(())
}
