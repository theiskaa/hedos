//! Reading device figures out of what the tools that know them print.

use super::{Device, DeviceKind};

const MIB: u64 = 1 << 20;

/// The devices in `llama-server --list-devices` output:
///
/// ```text
/// Available devices:
///   BLAS: Accelerate (0 MiB, 0 MiB free)
///   MTL0: Apple M5 Pro (53084 MiB, 53083 MiB free)
/// ```
///
/// Metal devices are unified; CUDA and ROCm ones discrete. The rest are left
/// out: BLAS and CPU are not accelerators, and Vulkan, SYCL and the remote
/// backends are ones Ollama, the engine behind every recommended chat model,
/// does not run on, so counting them would promise a speed it cannot give.
pub fn parse_list_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let (id, rest) = line.trim().split_once(": ")?;
            let kind = if id.starts_with("MTL") {
                DeviceKind::Unified
            } else if id.starts_with("CUDA") || id.starts_with("ROCm") {
                DeviceKind::Discrete
            } else {
                return None;
            };
            let open = rest.rfind('(')?;
            let name = rest[..open].trim();
            let figures = rest[open + 1..].trim_end().strip_suffix(')')?;
            let total = figures.split(',').next()?.trim().strip_suffix("MiB")?;
            let memory_bytes = total.trim().parse::<u64>().ok()?.checked_mul(MIB)?;
            (!name.is_empty() && memory_bytes > 0).then(|| Device {
                name: name.to_owned(),
                kind,
                memory_bytes,
            })
        })
        .collect()
}

/// The cards in `nvidia-smi --query-gpu=name,memory.total
/// --format=csv,noheader,nounits` output, one `name, MiB` line each. A line
/// whose memory reads `[N/A]` or anything else unparsable is skipped.
pub fn parse_nvidia_smi(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let (name, memory) = line.trim().rsplit_once(',')?;
            let memory_bytes = memory.trim().parse::<u64>().ok()?.checked_mul(MIB)?;
            let name = name.trim();
            (!name.is_empty() && memory_bytes > 0).then(|| Device {
                name: name.to_owned(),
                kind: DeviceKind::Discrete,
                memory_bytes,
            })
        })
        .collect()
}

/// An AMD card from its sysfs files: `vram_total` is the contents of
/// `mem_info_vram_total` (bytes, one line) and `name` that of `product_name`,
/// when the driver writes one.
pub fn amd_device(name: Option<&str>, vram_total: &str) -> Option<Device> {
    let memory_bytes = vram_total.trim().parse::<u64>().ok()?;
    if memory_bytes == 0 {
        return None;
    }
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("AMD GPU");
    Some(Device {
        name: name.to_owned(),
        kind: DeviceKind::Discrete,
        memory_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_macs_list_reads_as_one_unified_device() {
        let text = "Available devices:\n  BLAS: Accelerate (0 MiB, 0 MiB free)\n  MTL0: Apple M5 Pro (53084 MiB, 53083 MiB free)\n";
        assert_eq!(
            parse_list_devices(text),
            vec![Device {
                name: "Apple M5 Pro".to_owned(),
                kind: DeviceKind::Unified,
                memory_bytes: 53084 * MIB,
            }]
        );
    }

    #[test]
    fn cuda_and_rocm_cards_read_as_discrete() {
        let text = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24210 MiB, 23700 MiB free)\n  CUDA1: NVIDIA GeForce RTX 3090 (24135 MiB, 24000 MiB free)\n  ROCm0: AMD Radeon RX 7900 XTX (24560 MiB, 24000 MiB free)\n";
        let devices = parse_list_devices(text);
        assert_eq!(devices.len(), 3);
        assert!(devices.iter().all(|d| d.kind == DeviceKind::Discrete));
        assert_eq!(devices[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(devices[1].memory_bytes, 24135 * MIB);
    }

    #[test]
    fn a_name_with_parentheses_keeps_them() {
        let text = "  CUDA0: NVIDIA RTX A6000 (Ada) (49140 MiB, 48000 MiB free)";
        let devices = parse_list_devices(text);
        assert_eq!(devices[0].name, "NVIDIA RTX A6000 (Ada)");
        assert_eq!(devices[0].memory_bytes, 49140 * MIB);
    }

    #[test]
    fn vulkan_cpu_and_empty_lists_give_nothing() {
        let text = "Available devices:\n  Vulkan0: Intel(R) UHD Graphics (7680 MiB, 7000 MiB free)\n  CPU: AMD Ryzen 9 (64000 MiB, 60000 MiB free)\n";
        assert!(parse_list_devices(text).is_empty());
        assert!(parse_list_devices("Available devices:\n").is_empty());
        assert!(parse_list_devices("").is_empty());
    }

    #[test]
    fn garbage_gives_nothing() {
        for text in [
            "error: unknown argument --list-devices",
            "MTL0: Apple M1",
            "MTL0: Apple M1 (lots MiB, 0 MiB free)",
            "MTL0: (1024 MiB, 0 MiB free)",
            "MTL0: Apple M1 (0 MiB, 0 MiB free)",
            "MTL0: Apple M1 (1024 GiB, 0 MiB free)",
        ] {
            assert!(parse_list_devices(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn nvidia_smi_reads_one_card_per_line() {
        let text = "NVIDIA GeForce RTX 4090, 24564\nNVIDIA A100-SXM4-80GB, 81920\n";
        let devices = parse_nvidia_smi(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(devices[0].memory_bytes, 24564 * MIB);
        assert_eq!(devices[1].memory_bytes, 81920 * MIB);
    }

    #[test]
    fn nvidia_smi_skips_what_it_cannot_read() {
        assert!(parse_nvidia_smi("NVIDIA T4, [N/A]\n").is_empty());
        assert!(parse_nvidia_smi("").is_empty());
        assert!(parse_nvidia_smi("No devices were found").is_empty());
        assert!(parse_nvidia_smi(", 1024").is_empty());
    }

    #[test]
    fn an_amd_card_reads_its_bytes_and_name() {
        let device = amd_device(Some("Radeon RX 7900 XTX\n"), "25753026560\n").unwrap();
        assert_eq!(device.name, "Radeon RX 7900 XTX");
        assert_eq!(device.memory_bytes, 25_753_026_560);
        assert_eq!(amd_device(None, "1024").unwrap().name, "AMD GPU");
        assert_eq!(amd_device(Some("  "), "1024").unwrap().name, "AMD GPU");
        assert!(amd_device(None, "0").is_none());
        assert!(amd_device(None, "n/a").is_none());
    }
}
