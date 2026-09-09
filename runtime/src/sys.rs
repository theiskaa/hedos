//! The one binding to `sysctlbyname`, and the two shapes the runtime reads
//! through it. Two callers wanted the same libc symbol; declaring it twice in
//! one crate is how the two would drift.

#![cfg(target_os = "macos")]

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

/// The longest string value this will read back. A name is short; anything
/// this size is not one.
const MAX_STRING: usize = 1024;

unsafe extern "C" {
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
}

/// The `u64` value of `name`, or `None` where the call failed.
pub(crate) fn u64_value(name: &CStr) -> Option<u64> {
    let mut value: u64 = 0;
    let mut length = std::mem::size_of::<u64>();
    // SAFETY: `name` is a valid NUL-terminated C string; `value`/`length` are
    // valid, correctly-sized out-parameters, and the call writes at most
    // `length` bytes into `value`. A non-zero return means failure.
    let result = unsafe {
        sysctlbyname(
            name.as_ptr(),
            std::ptr::from_mut(&mut value).cast(),
            std::ptr::from_mut(&mut length),
            std::ptr::null(),
            0,
        )
    };
    (result == 0).then_some(value)
}

/// The string value of `name`, trimmed, or `None` where the call failed or the
/// value was empty.
pub(crate) fn string_value(name: &CStr) -> Option<String> {
    let mut length: usize = 0;
    // SAFETY: `name` is a valid NUL-terminated C string; a null `oldp` with a
    // valid `oldlenp` asks only for the value's length, which is all this
    // reads. A non-zero return means failure.
    let sized = unsafe {
        sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::from_mut(&mut length),
            std::ptr::null(),
            0,
        )
    };
    if sized != 0 || length == 0 || length > MAX_STRING {
        return None;
    }
    let mut buffer = vec![0u8; length];
    // SAFETY: `buffer` holds exactly the `length` bytes the call above asked
    // for, and `length` is passed by pointer so the call cannot write past it.
    let read = unsafe {
        sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            std::ptr::from_mut(&mut length),
            std::ptr::null(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(length);
    let text = String::from_utf8(buffer).ok()?;
    let name = text.trim_end_matches('\0').trim();
    (!name.is_empty()).then(|| name.to_owned())
}
