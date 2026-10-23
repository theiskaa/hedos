//! The one binding to `sysctlbyname`, and the two shapes the runtime reads
//! through it. Two callers wanted the same libc symbol; declaring it twice in
//! one crate is how the two would drift. Beside it, the binding to the Mach
//! host statistics the free memory is read from.

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
    fn mach_host_self() -> libc::mach_port_t;
    fn host_page_size(host: libc::mach_port_t, size: *mut libc::vm_size_t) -> libc::kern_return_t;
    fn host_statistics64(
        host: libc::mach_port_t,
        flavor: c_int,
        info: *mut libc::integer_t,
        count: *mut libc::mach_msg_type_number_t,
    ) -> libc::kern_return_t;
}

/// This task's send right to the host port, taken once: each
/// `mach_host_self` call adds a reference that nothing here would release.
fn host_port() -> libc::mach_port_t {
    static PORT: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    // SAFETY: `mach_host_self` takes no arguments and only returns a port
    // name for this task.
    *PORT.get_or_init(|| unsafe { mach_host_self() })
}

/// The bytes of memory free or reclaimable without swapping right now: the
/// free and inactive pages, as `psutil` reads macOS's available memory.
/// `None` where the kernel would not say.
pub(crate) fn reclaimable_memory() -> Option<u64> {
    // SAFETY: `vm_statistics64` is plain integers, for which all zeroes is a
    // valid value.
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `stats` is a `vm_statistics64` and `count` says how many
    // `integer_t` it holds, which bounds what the call writes into it.
    let result = unsafe {
        host_statistics64(
            host_port(),
            libc::HOST_VM_INFO64,
            std::ptr::from_mut(&mut stats).cast(),
            std::ptr::from_mut(&mut count),
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    // The kernel's page, which the counts are in: a process under Rosetta
    // sees 4 KiB pages where the kernel's are 16 KiB.
    let mut page: libc::vm_size_t = 0;
    // SAFETY: `page` is a valid out-parameter of the size the call writes.
    if unsafe { host_page_size(host_port(), std::ptr::from_mut(&mut page)) } != libc::KERN_SUCCESS {
        return None;
    }
    let page = u64::try_from(page).ok()?;
    let pages = u64::from(stats.free_count) + u64::from(stats.inactive_count);
    pages.checked_mul(page)
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
