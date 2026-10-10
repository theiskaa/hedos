//! The one question asked of Metal: how much memory the system's GPU may
//! give a model. Every Metal engine (llama.cpp, Ollama's runner, MLX) budgets
//! against `recommendedMaxWorkingSetSize`, and no share of RAM reproduces it:
//! on a 64 GiB M5 Pro it is 53084 MiB, 81 %, where the usual two-thirds or
//! three-quarters rule says 48 GiB.

use std::ffi::{CStr, c_void};
use std::os::raw::c_char;

type Id = *mut c_void;
type Sel = *const c_void;

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

#[link(name = "objc")]
unsafe extern "C" {
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_release(object: Id);
}

/// The default GPU's recommended working set in bytes, when it shares the
/// system's memory. `None` where there is no Metal device, or where it has
/// memory of its own (an Intel Mac's AMD card), which no engine here runs a
/// model on.
pub(crate) fn recommended_working_set() -> Option<u64> {
    // SAFETY: a plain C function with no arguments. It returns a retained
    // device, or null when the system has none.
    let device = unsafe { MTLCreateSystemDefaultDevice() };
    if device.is_null() {
        return None;
    }
    // SAFETY: `device` is live until the release below. `hasUnifiedMemory`
    // is a no-argument getter returning `BOOL`, one byte on every Apple
    // architecture, and `recommendedMaxWorkingSetSize` one returning
    // `uint64_t`.
    let (unified, bytes) = unsafe {
        (
            message::<u8>(device, c"hasUnifiedMemory") != 0,
            message::<u64>(device, c"recommendedMaxWorkingSetSize"),
        )
    };
    // SAFETY: `device` came retained from a Create function and is released
    // exactly once, here, after its last use.
    unsafe { objc_release(device) };
    (unified && bytes > 0).then_some(bytes)
}

/// Send `selector` to `receiver` and read back its value as `T`.
///
/// # Safety
///
/// `receiver` must be a live Objective-C object that responds to `selector`,
/// and `selector` must name a method that takes no arguments and returns a
/// scalar with exactly `T`'s size and calling convention.
unsafe fn message<T>(receiver: Id, selector: &CStr) -> T {
    // SAFETY: `selector` is a valid NUL-terminated string; registering a name
    // that already exists returns the existing selector.
    let selector = unsafe { sel_registerName(selector.as_ptr()) };
    // SAFETY: `objc_msgSend` must be called through a pointer cast to the
    // method's own signature, which the caller vouches is `(Id, Sel) -> T`.
    unsafe {
        let send: unsafe extern "C" fn(Id, Sel) -> T =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(receiver, selector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Mac with a GPU sharing its memory reports a working set within
    /// that memory; a virtual machine without one reports none, and reading
    /// it there is no failure.
    #[test]
    fn a_working_set_when_there_is_one_lies_within_memory() {
        let memory = crate::sys::u64_value(c"hw.memsize").unwrap();
        if let Some(bytes) = recommended_working_set() {
            assert!(bytes > 0 && bytes <= memory, "{bytes} of {memory}");
        }
    }
}
