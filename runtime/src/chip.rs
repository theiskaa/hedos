//! The machine's processor, as it names itself. Shown beside a bench's
//! figures, since a rate means nothing without the chip that produced it.

/// The processor's own name (`Apple M3 Max`, `Intel(R) Core(TM) i7-...`), or
/// `None` where it cannot be read.
pub fn name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos_brand_string()
    }
    #[cfg(target_os = "linux")]
    {
        linux_model_name()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn macos_brand_string() -> Option<String> {
    crate::sys::string_value(c"machdep.cpu.brand_string")
}

#[cfg(target_os = "linux")]
fn linux_model_name() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, name)| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}
