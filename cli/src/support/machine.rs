//! Machine facts the commands need, read from the same source the governor uses
//! so every surface agrees on one number.

use runtime::governor::GovernorConfig;

/// The machine's total memory in bytes, for fit assessment. Reads the governor's
/// detected total (never below 1 MiB) so `hedos ls`, the picker, and the install
/// catalog all judge fit against the identical budget.
pub fn memory_budget_bytes() -> u64 {
    GovernorConfig::detect().total_memory_mb.max(1) as u64 * 1024 * 1024
}
