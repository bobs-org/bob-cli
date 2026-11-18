//! Shared Darwin notification transport; callers keep ownership of their receipts.
#[cfg(target_os = "macos")]
pub(crate) fn post(name: &str) -> bool {
    use std::ffi::CString;
    use std::os::raw::c_char;
    #[link(name = "System")]
    unsafe extern "C" {
        fn notify_post(name: *const c_char) -> i32;
    }
    let Ok(name) = CString::new(name) else {
        return false;
    };
    unsafe { notify_post(name.as_ptr()) == 0 }
}
