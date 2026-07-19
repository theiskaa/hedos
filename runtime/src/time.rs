//! Small time helpers shared across the runtime — re-exported from the kernel so
//! there is one epoch-millisecond clock implementation across the workspace.

pub(crate) use kernel::time::now_millis;
