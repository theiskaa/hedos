//! This machine as the commands judge it, read through the runtime's probe so
//! every surface agrees on one set of figures.

use kernel::machine::Machine;

/// This machine: its memory, the accelerator a model would run on, the
/// engines installed, and the free disk where pulls land.
pub fn machine() -> Machine {
    runtime::machine::probe()
}
