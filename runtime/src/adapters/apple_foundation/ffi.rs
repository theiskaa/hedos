//! The FFI backend over the Swift FoundationModels shim: it loads
//! `libhedos_apple_shim.dylib`, probes availability, and streams generations
//! through the C ABI documented at the top of `shim-apple/shim.swift`. Only
//! compiled on macOS; every failure to load degrades to the missing backend
//! rather than erroring, so a binary built without the shim (or run on a Mac
//! that cannot serve it) simply has no Apple model.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use kernel::capabilities::ChatMessage;
use libloading::Library;
use tokio::sync::mpsc::UnboundedSender;

use super::backend::{
    AppleFoundationBackend, BuiltinAvailability, BuiltinEvent, BuiltinEventStream, BuiltinOptions,
    MissingAppleBackend,
};
use super::wire::{done_event, request_json};
use crate::adapters::{RuntimeError, RuntimeStream};

/// The shim ABI this backend speaks (`hedos_af_abi_version`).
const SUPPORTED_ABI: u32 = 1;

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type AvailabilityFn = unsafe extern "C" fn() -> i32;
type EventCallback = unsafe extern "C" fn(*mut c_void, i32, *const c_char);
type StreamFn = unsafe extern "C" fn(*const c_char, *mut c_void, EventCallback) -> u64;
type CancelFn = unsafe extern "C" fn(u64);

/// The loaded shim: the raw entry points plus the library that keeps them
/// alive. The function pointers are copied out of their [`Library`] symbols,
/// which is sound only because the library is held alongside them for the
/// process lifetime (the shim lives in a `'static` [`OnceLock`]).
struct Shim {
    availability: AvailabilityFn,
    stream: StreamFn,
    cancel: CancelFn,
    _library: Library,
}

/// The apple-foundation backend for this build: the FFI backend when the shim
/// dylib loads and speaks our ABI, else the missing placeholder.
pub fn loaded_apple_backend() -> Arc<dyn AppleFoundationBackend> {
    match shim() {
        Some(shim) => Arc::new(FfiAppleBackend { shim }),
        None => Arc::new(MissingAppleBackend),
    }
}

fn shim() -> Option<&'static Shim> {
    static SHIM: OnceLock<Option<Shim>> = OnceLock::new();
    SHIM.get_or_init(|| candidate_paths().iter().find_map(|path| load(path)))
        .as_ref()
}

/// Where the shim may live, most-specific first: an explicit override, next
/// to the running binary, then the path baked in by the build script (which
/// only holds on the machine that built this binary).
fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(overridden) = std::env::var_os("HEDOS_APPLE_SHIM") {
        paths.push(PathBuf::from(overridden));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("libhedos_apple_shim.dylib"));
    }
    // Empty when the building toolchain had no FoundationModels SDK.
    let baked = env!("HEDOS_APPLE_SHIM_BUILT_DYLIB");
    if !baked.is_empty() {
        paths.push(PathBuf::from(baked));
    }
    paths
}

/// Load and verify a shim at `path`: every symbol must resolve and the ABI
/// version must match, else the candidate is skipped.
fn load(path: &Path) -> Option<Shim> {
    if !path.is_file() {
        return None;
    }
    // SAFETY: the dylib is hedos's own shim; loading runs no initializer
    // beyond Swift runtime setup, and each symbol is verified to exist before
    // use with the signature the shim's ABI contract documents.
    unsafe {
        let library = Library::new(path).ok()?;
        let version = *library.get::<AbiVersionFn>(b"hedos_af_abi_version").ok()?;
        if version() != SUPPORTED_ABI {
            return None;
        }
        let availability = *library
            .get::<AvailabilityFn>(b"hedos_af_availability")
            .ok()?;
        let stream = *library.get::<StreamFn>(b"hedos_af_stream").ok()?;
        let cancel = *library.get::<CancelFn>(b"hedos_af_cancel").ok()?;
        Some(Shim {
            availability,
            stream,
            cancel,
            _library: library,
        })
    }
}

/// The state the shim hands back on every callback: the channel into the
/// consumer's event stream. Boxed across the FFI boundary; freed exactly once,
/// on the terminal event.
struct CallbackState {
    tx: UnboundedSender<Result<BuiltinEvent, RuntimeError>>,
}

/// The C callback the shim invokes per event: routes each into the backend
/// stream and frees the boxed state on the terminal event.
unsafe extern "C" fn on_event(ctx: *mut c_void, kind: i32, payload: *const c_char) {
    // SAFETY: `ctx` is the `Box<CallbackState>` handed to `hedos_af_stream`;
    // the shim invokes this callback sequentially per generation and never
    // after the terminal event, so the shared reference cannot alias a free.
    let state = unsafe { &*(ctx as *const CallbackState) };
    let text = if payload.is_null() {
        String::new()
    } else {
        // SAFETY: the contract guarantees a NUL-terminated payload valid for
        // the duration of this call.
        unsafe { CStr::from_ptr(payload) }
            .to_string_lossy()
            .into_owned()
    };
    let terminal = match kind {
        0 => {
            let _ = state.tx.send(Ok(BuiltinEvent::Snapshot(text)));
            false
        }
        1 => {
            let _ = state.tx.send(Ok(done_event(&text)));
            true
        }
        2 => {
            let _ = state.tx.send(Err(RuntimeError::Failed(text)));
            true
        }
        3 => {
            let _ = state.tx.send(Err(RuntimeError::Cancelled));
            true
        }
        // An unknown kind would mean an ABI drift the version guard missed;
        // dropping the event keeps the stream alive for the terminal one.
        _ => false,
    };
    if terminal {
        // SAFETY: the terminal event is the shim's last touch of `ctx`
        // (documented contract), so ownership returns here exactly once.
        drop(unsafe { Box::from_raw(ctx as *mut CallbackState) });
    }
}

/// The backend over a loaded shim.
struct FfiAppleBackend {
    shim: &'static Shim,
}

impl AppleFoundationBackend for FfiAppleBackend {
    fn availability(&self) -> BuiltinAvailability {
        // SAFETY: a verified no-argument entry point of the loaded shim.
        match unsafe { (self.shim.availability)() } {
            0 => BuiltinAvailability::Available,
            1 => BuiltinAvailability::NotEnabled,
            3 => BuiltinAvailability::NotEligible,
            // 2 is "not ready"; an unknown code degrades the same way.
            _ => BuiltinAvailability::NotReady,
        }
    }

    fn stream(&self, messages: Vec<ChatMessage>, options: BuiltinOptions) -> BuiltinEventStream {
        let (tx, stream) = RuntimeStream::channel();
        let Ok(request) = CString::new(request_json(&messages, &options)) else {
            return RuntimeStream::failed(RuntimeError::Failed(
                "the generation request could not be encoded".to_owned(),
            ));
        };
        let state = Box::into_raw(Box::new(CallbackState { tx: tx.clone() }));
        // SAFETY: `request` lives across the call (the shim decodes it before
        // returning); `state` stays valid until the terminal event frees it in
        // `on_event`, per the shim's contract.
        let handle =
            unsafe { (self.shim.stream)(request.as_ptr(), state as *mut c_void, on_event) };
        if handle == 0 {
            // Decode failure: the error event already arrived synchronously
            // and freed `state`; the stream will yield it.
            return stream;
        }
        let cancel = self.shim.cancel;
        tokio::spawn(async move {
            // Fires when the consumer drops the stream — including long after
            // a normal finish, where cancelling a finished handle is a no-op.
            tx.closed().await;
            // SAFETY: a verified entry point; any u64 is a valid argument.
            unsafe { cancel(handle) };
        });
        stream
    }
}
