//! llama.cpp as hedos installs it on request: the one release a hedos build
//! pins, its builds per machine, which build suits a machine, and reading the
//! version a `llama-server` reports. Nothing here downloads or runs anything.

use serde::{Deserialize, Serialize};

pub use super::{INSTALLED_FILE, InstalledFile};
use crate::machine::{DevicesFrom, Machine, Os};

/// The llama.cpp release this hedos installs: its version, the build tag its
/// binaries are published under, and each build with its pinned sha256. The
/// Linux CPU builds are hedos's own, built from the tag's source (see
/// [`Publisher::Hedos`]); every other build is llama.cpp's.
pub const PINNED: Release = Release {
    version: "0.7.0",
    build: "b11669",
    source: SourceTarball {
        commit: "620d2225c",
        sha256: "e06ba4fa263e30fdb9f0e65f00a8965202ddaa0c51f548a67e1f9b2164d04efc",
    },
    assets: &[
        Asset {
            os: Os::Macos,
            arch: "aarch64",
            variant: Variant::Metal,
            file: "llama-b11669-bin-macos-arm64.tar.gz",
            sha256: "dbed55493d72bc1ba2babc8770d669c2289b5bc4ee9fb2a4dfc158d45b7d13fd",
            bytes: 12_191_582,
            cudart: None,
            publisher: Publisher::LlamaCpp,
        },
        Asset {
            os: Os::Macos,
            arch: "x86_64",
            variant: Variant::Cpu,
            file: "llama-b11669-bin-macos-x64.tar.gz",
            sha256: "cab28238d78158109b48deb27b05d50e0dcef56275d2e76da9ca8281526c7532",
            bytes: 11_677_404,
            cudart: None,
            publisher: Publisher::LlamaCpp,
        },
        Asset {
            os: Os::Linux,
            arch: "x86_64",
            variant: Variant::Cpu,
            file: "llama-b11669-bin-linux-x86_64.tar.gz",
            sha256: "2f28233ef38a1ee6afe59d612634b6fa6c712fe16de0a7b22f9e700d76f25a8a",
            bytes: 17_676_284,
            cudart: None,
            publisher: Publisher::Hedos,
        },
        Asset {
            os: Os::Linux,
            arch: "aarch64",
            variant: Variant::Cpu,
            file: "llama-b11669-bin-linux-aarch64.tar.gz",
            sha256: "9d67ff79f063e01033153cc068be6dd69717f841859269f960ec469f6fec2414",
            bytes: 13_043_665,
            cudart: None,
            publisher: Publisher::Hedos,
        },
        Asset {
            os: Os::Linux,
            arch: "x86_64",
            variant: Variant::Vulkan,
            file: "llama-b11669-bin-ubuntu-vulkan-x64.tar.gz",
            sha256: "e8709e5ba6c1b093dea7f3de57f4aba842f9a188fba6b465782d65d6fcfa4d9f",
            bytes: 30_497_889,
            cudart: None,
            publisher: Publisher::LlamaCpp,
        },
        Asset {
            os: Os::Linux,
            arch: "aarch64",
            variant: Variant::Vulkan,
            file: "llama-b11669-bin-ubuntu-vulkan-arm64.tar.gz",
            sha256: "d283522e479c0d532e3a04d715dd852ffb2d95925caa6d41ef2ba53a65e46a51",
            bytes: 24_187_561,
            cudart: None,
            publisher: Publisher::LlamaCpp,
        },
        Asset {
            os: Os::Linux,
            arch: "x86_64",
            variant: Variant::Cuda12,
            file: "llama-b11669-bin-ubuntu-cuda-12.8-x64.tar.gz",
            sha256: "4703ed1a3cd6e8504affb6cb9866f501e96d634d68a002dc9a8251c9e2819b49",
            bytes: 172_525_113,
            cudart: Some(Runtime {
                file: "cudart-llama-b11669-bin-ubuntu-cuda-12.8-x64.tar.gz",
                sha256: "ab3eda89a82096bcba00b6448218c3c11eec2016b0f38d64884dc29736549f26",
                bytes: 594_377_811,
            }),
            publisher: Publisher::LlamaCpp,
        },
        Asset {
            os: Os::Linux,
            arch: "aarch64",
            variant: Variant::Cuda13,
            file: "llama-b11669-bin-ubuntu-cuda-13.4-arm64.tar.gz",
            sha256: "19902edd0c02942ac736c2dea5e104ff219a196a5cd07f125a7adcd44c103e6b",
            bytes: 149_286_186,
            cudart: Some(Runtime {
                file: "cudart-llama-b11669-bin-ubuntu-cuda-13.4-arm64.tar.gz",
                sha256: "6c81453d16a574eef3273662a7a8ceafa3537699a5b2ffec3fc853eb1a08f67c",
                bytes: 552_521_396,
            }),
            publisher: Publisher::LlamaCpp,
        },
        Asset {
            os: Os::Linux,
            arch: "x86_64",
            variant: Variant::Rocm,
            file: "llama-b11669-bin-ubuntu-rocm-10.0-x64.tar.gz",
            sha256: "02d36f06accd8fdcb2d41078361b21fb2df5d139f169bf4efcb8424a7dbbef01",
            bytes: 245_070_738,
            cudart: None,
            publisher: Publisher::LlamaCpp,
        },
    ],
};

/// The first build that serves decision models (`/v1/systemone`): llama.cpp
/// 0.6.0's.
pub const DECISION_MODELS_BUILD: u64 = 11429;

/// The build number a llama.cpp build tag names (`11669` for `b11669`):
/// a `b` and digits, nothing else.
pub fn parse_build_tag(tag: &str) -> Option<u64> {
    let digits = tag.strip_prefix('b')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Where llama.cpp's release assets are published.
const RELEASES: &str = "https://github.com/ggml-org/llama.cpp/releases/download";

/// Where hedos's own builds of a llama.cpp release are published, each
/// release's under the tag `llama.cpp-<build>`.
const HEDOS_RELEASES: &str = "https://github.com/theiskaa/hedos/releases/download";

/// Where a tag's source tarball is published.
const SOURCES: &str = "https://github.com/ggml-org/llama.cpp/archive/refs/tags";

/// A llama.cpp release as hedos pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    /// The release's version (`0.7.0`).
    pub version: &'static str,
    /// The build tag its binaries are published under (`b11669`).
    pub build: &'static str,
    /// The tag's source, which hedos builds its own Linux builds from.
    pub source: SourceTarball,
    /// Every build of it hedos knows how to install.
    pub assets: &'static [Asset],
}

/// A release's source tarball as hedos pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceTarball {
    /// The commit the tag names, as `--version` reports it (short).
    pub commit: &'static str,
    /// The tarball's sha256, lowercase hex.
    pub sha256: &'static str,
}

impl Release {
    /// The build of this release for `variant` on `os`/`arch`, if there is one.
    pub fn asset(&self, os: Os, arch: &str, variant: Variant) -> Option<&'static Asset> {
        self.assets
            .iter()
            .find(|asset| asset.os == os && asset.arch == arch && asset.variant == variant)
    }

    /// The variants this release has a build of for `os`/`arch` that start
    /// on a system whose glibc is `glibc`.
    pub fn variants_starting(&self, os: Os, arch: &str, glibc: Option<&str>) -> Vec<Variant> {
        self.assets
            .iter()
            .filter(|asset| asset.os == os && asset.arch == arch && asset.blocked(glibc).is_none())
            .map(|asset| asset.variant)
            .collect()
    }

    /// The build number of the tag (`11669` for `b11669`).
    pub fn build_number(&self) -> Option<u64> {
        parse_build_tag(self.build)
    }

    /// Where `file`, one of this release's assets that `publisher` publishes,
    /// is downloaded from.
    pub fn url(&self, publisher: Publisher, file: &str) -> String {
        match publisher {
            Publisher::LlamaCpp => format!("{RELEASES}/{}/{file}", self.build),
            Publisher::Hedos => format!("{HEDOS_RELEASES}/llama.cpp-{}/{file}", self.build),
        }
    }

    /// Where the tag's source tarball is downloaded from.
    pub fn source_url(&self) -> String {
        format!("{SOURCES}/{}.tar.gz", self.build)
    }
}

/// One build of a release: an archive of `llama-server` and the libraries it
/// loads, for one machine and accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// The operating system it runs on.
    pub os: Os,
    /// The CPU architecture it runs on (`aarch64`, `x86_64`).
    pub arch: &'static str,
    /// The accelerator it is built for.
    pub variant: Variant,
    /// The archive's file name on the release.
    pub file: &'static str,
    /// The archive's sha256, lowercase hex.
    pub sha256: &'static str,
    /// The archive's size in bytes, as the release lists it.
    pub bytes: u64,
    /// The CUDA runtime a CUDA build needs beside it, published as an archive
    /// of its own.
    pub cudart: Option<Runtime>,
    /// Who builds and publishes it.
    pub publisher: Publisher,
}

impl Asset {
    /// What installing it downloads: the build and its runtime.
    pub fn download_bytes(&self) -> u64 {
        self.bytes + self.cudart.map_or(0, |runtime| runtime.bytes)
    }

    /// Why it cannot start on a system whose glibc is `glibc`, when it
    /// cannot: llama.cpp's own Linux builds need 2.38, hedos's do not.
    pub fn blocked(&self, glibc: Option<&str>) -> Option<String> {
        if self.os != Os::Linux || self.publisher != Publisher::LlamaCpp {
            return None;
        }
        published_build_blocked(glibc?)
    }
}

/// Who builds and publishes a build of a llama.cpp release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Publisher {
    /// llama.cpp itself, on its GitHub releases.
    LlamaCpp,
    /// hedos, from the release's pinned source, on hedos's GitHub releases.
    /// llama.cpp's own Linux archives are built on Ubuntu 24.04 and need
    /// glibc 2.38, GCC 13's libstdc++, libgomp and OpenSSL 3, so they do not
    /// start on Ubuntu 22.04 or Debian 12; hedos's CPU builds are made in
    /// manylinux_2_28 (glibc 2.28) with the C++ runtime linked in and no
    /// OpenMP or OpenSSL (`.github/build-llama-cpp.sh`).
    Hedos,
}

/// An archive of libraries a build loads, unpacked beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    /// The archive's file name on the release.
    pub file: &'static str,
    /// The archive's sha256, lowercase hex.
    pub sha256: &'static str,
    /// The archive's size in bytes, as the release lists it.
    pub bytes: u64,
}

/// The accelerator a llama.cpp build runs models on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Variant {
    /// Apple's GPU, through Metal.
    Metal,
    /// The processor alone.
    Cpu,
    /// Any GPU with a Vulkan driver.
    Vulkan,
    /// NVIDIA's GPUs, through CUDA 12.
    #[serde(rename = "cuda-12")]
    Cuda12,
    /// NVIDIA's GPUs, through CUDA 13.
    #[serde(rename = "cuda-13")]
    Cuda13,
    /// AMD's GPUs, through ROCm, which has to be installed.
    Rocm,
}

impl Variant {
    /// Every variant, in the order they are listed.
    pub const ALL: [Variant; 6] = [
        Variant::Metal,
        Variant::Cpu,
        Variant::Vulkan,
        Variant::Cuda12,
        Variant::Cuda13,
        Variant::Rocm,
    ];

    /// The stable string form, as `--variant` takes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Metal => "metal",
            Variant::Cpu => "cpu",
            Variant::Vulkan => "vulkan",
            Variant::Cuda12 => "cuda-12",
            Variant::Cuda13 => "cuda-13",
            Variant::Rocm => "rocm",
        }
    }

    /// The variant `name` spells, in any case.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variant| variant.as_str().eq_ignore_ascii_case(name))
    }

    /// The build that suits `machine`, whose glibc is `glibc`: Metal on a
    /// Mac with Apple's chip, the CPU build on an Intel Mac, whose GPU no
    /// engine here runs a model on; on Linux, CUDA for NVIDIA cards (12 on
    /// x86_64, 13 where the release only builds that for arm64), Vulkan for
    /// AMD cards (ROCm has to be installed on its own, so it is taken only
    /// when asked for), and the CPU build otherwise, or where the build that
    /// suits the cards cannot start on this glibc. `None` where the release
    /// has no build at all.
    pub fn default_for(machine: &Machine, glibc: Option<&str>) -> Option<Self> {
        let arch = machine.arch.as_str();
        let chosen = match (machine.os, arch, machine.devices_from) {
            (Os::Macos, "aarch64", _) => Variant::Metal,
            (Os::Linux, "x86_64", DevicesFrom::NvidiaSmi) => Variant::Cuda12,
            (Os::Linux, "aarch64", DevicesFrom::NvidiaSmi) => Variant::Cuda13,
            (Os::Linux, _, DevicesFrom::AmdSysfs) => Variant::Vulkan,
            _ => Variant::Cpu,
        };
        let suits = PINNED.asset(machine.os, arch, chosen)?;
        if suits.blocked(glibc).is_some() {
            return PINNED
                .asset(machine.os, arch, Variant::Cpu)
                .map(|asset| asset.variant);
        }
        Some(suits.variant)
    }
}

/// The build hedos carries inside itself for `os`/`arch`: Metal on a Mac with
/// Apple's chip, the CPU build on an Intel Mac and on Linux, where a GPU build
/// needs a driver or a runtime the machine may not have. `None` where hedos
/// carries none.
pub fn bundled_variant(os: Os, arch: &str) -> Option<Variant> {
    match (os, arch) {
        (Os::Macos, "aarch64") => Some(Variant::Metal),
        (Os::Macos | Os::Linux, "x86_64") | (Os::Linux, "aarch64") => Some(Variant::Cpu),
        _ => None,
    }
}

/// The oldest glibc llama.cpp's own Linux archives start on: they are built on
/// Ubuntu 24.04.
const PUBLISHED_LINUX_GLIBC: (u32, u32) = (2, 38);

/// A glibc version (`2.35`) as its major and minor numbers.
fn parse_glibc(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Why llama.cpp's own Linux builds cannot start on a system whose glibc is
/// `glibc`, when they cannot. hedos's own builds start there
/// ([`Asset::blocked`]).
fn published_build_blocked(glibc: &str) -> Option<String> {
    let found = parse_glibc(glibc)?;
    (found < PUBLISHED_LINUX_GLIBC).then(|| {
        let (major, minor) = PUBLISHED_LINUX_GLIBC;
        format!(
            "llama.cpp's own Linux builds need glibc {major}.{minor} or newer (Ubuntu 24.04, Debian 13, Fedora 39), and this system has {glibc}"
        )
    })
}

/// What a `llama-server` that would not start said, read as the reason: the
/// shared library the loader could not find (and the package that has it),
/// or the glibc it needs.
pub fn loader_failure(said: &str) -> Option<String> {
    if let Some(rest) = said.split("error while loading shared libraries: ").nth(1) {
        let library = rest.split(':').next()?.trim();
        let package = match library {
            "libgomp.so.1" => Some("libgomp1 on Debian and Ubuntu, libgomp on Fedora"),
            "libssl.so.3" | "libcrypto.so.3" => {
                Some("libssl3 on Debian and Ubuntu, openssl-libs on Fedora")
            }
            "libvulkan.so.1" => Some("libvulkan1 on Debian and Ubuntu, vulkan-loader on Fedora"),
            _ => None,
        };
        return Some(match package {
            Some(package) => {
                format!("it needs {library}, which this system lacks: install {package}")
            }
            None => format!("it needs {library}, which this system lacks"),
        });
    }
    let needs = said.split("version `GLIBC_").nth(1)?;
    let version = needs.split('\'').next()?;
    Some(format!(
        "it needs glibc {version} or newer, which this system lacks"
    ))
}

/// How a build hedos keeps came to be there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallSource {
    /// Unpacked from the build hedos carries inside itself.
    Bundled,
    /// Fetched because someone asked for it (`hedos engines install`).
    #[default]
    Requested,
}

/// What hedos writes beside a `llama-server` it installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The release's version.
    pub version: String,
    /// The build tag.
    pub build: String,
    /// The accelerator it is built for.
    pub variant: Variant,
    /// The sha256 of the archive it came from.
    pub sha256: String,
    /// When it was installed, in milliseconds since the epoch.
    pub installed_at: i64,
    /// How it came to be there; a record written before hedos carried a
    /// build of its own was installed on request.
    #[serde(default)]
    pub source: InstallSource,
    /// Every file it was put in place with, so a build that lost or cut short
    /// one of them since is not taken for whole.
    #[serde(default)]
    pub files: Vec<InstalledFile>,
}

/// The directory, under hedos's llama.cpp root, a build of `build` for
/// `variant` from `source` is kept in: `b11669-metal`, or `b11669-metal-bundled`
/// for the one hedos carries, so the two never take each other's place.
pub fn install_dir_name(build: &str, variant: Variant, source: InstallSource) -> String {
    match source {
        InstallSource::Requested => format!("{build}-{}", variant.as_str()),
        InstallSource::Bundled => format!("{build}-{}-bundled", variant.as_str()),
    }
}

/// The version a `llama-server --version` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaVersion {
    /// The build number.
    pub build: u64,
    /// The release version, which builds since 0.x name (`0.7.0-dev`).
    pub version: Option<String>,
}

impl LlamaVersion {
    /// The version as a person reads it: `0.7.0-dev (build 11756)`, or
    /// `build 10050` for a build that names no release.
    pub fn label(&self) -> String {
        match &self.version {
            Some(version) => format!("{version} (build {})", self.build),
            None => format!("build {}", self.build),
        }
    }
}

/// The version in what `llama-server --version` printed: its `version:` line,
/// either `version: 0.7.0-dev (build 11756, commit d80619397)` or, from builds
/// before llama.cpp named releases, `version: 10050 (b15ca938a)`.
pub fn parse_llama_version(output: &str) -> Option<LlamaVersion> {
    let line = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("version:"))?
        .trim();
    let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
    if let Ok(build) = head.parse::<u64>() {
        return Some(LlamaVersion {
            build,
            version: None,
        });
    }
    let build = rest
        .trim_start_matches('(')
        .split([',', ')'])
        .find_map(|part| part.trim().strip_prefix("build "))?
        .trim()
        .parse()
        .ok()?;
    Some(LlamaVersion {
        build,
        version: Some(head.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::testing::{apple_silicon, card, linux};

    #[test]
    fn both_version_shapes_read_and_anything_else_does_not() {
        let current = "load_backend: loaded\nversion: 0.7.0-dev (build 11756, commit d80619397)\nbuilt with AppleClang 21.0.0";
        assert_eq!(
            parse_llama_version(current),
            Some(LlamaVersion {
                build: 11756,
                version: Some("0.7.0-dev".to_owned())
            })
        );
        assert_eq!(
            parse_llama_version("version: 10050 (b15ca938a)\nbuilt with AppleClang"),
            Some(LlamaVersion {
                build: 10050,
                version: None
            })
        );
        assert_eq!(parse_llama_version("no version here"), None);
        assert_eq!(parse_llama_version("version: soon"), None);
        assert_eq!(
            parse_llama_version(current).unwrap().label(),
            "0.7.0-dev (build 11756)"
        );
    }

    #[test]
    fn each_machine_gets_the_build_that_suits_it() {
        assert_eq!(
            Variant::default_for(&apple_silicon(64, 53084), None),
            Some(Variant::Metal)
        );
        let intel_mac = Machine {
            arch: "x86_64".to_owned(),
            ..apple_silicon(16, 10923)
        };
        assert_eq!(Variant::default_for(&intel_mac, None), Some(Variant::Cpu));
        let nvidia = linux(64, vec![card("RTX 4090", 24)]);
        assert_eq!(Variant::default_for(&nvidia, None), Some(Variant::Cuda12));
        let nvidia_arm = Machine {
            arch: "aarch64".to_owned(),
            ..linux(64, vec![card("GH200", 96)])
        };
        assert_eq!(
            Variant::default_for(&nvidia_arm, None),
            Some(Variant::Cuda13)
        );
        let amd = Machine {
            devices_from: DevicesFrom::AmdSysfs,
            ..linux(64, vec![card("RX 7900", 24)])
        };
        assert_eq!(Variant::default_for(&amd, None), Some(Variant::Vulkan));
        let plain = Machine {
            devices_from: DevicesFrom::None,
            ..linux(32, Vec::new())
        };
        assert_eq!(Variant::default_for(&plain, None), Some(Variant::Cpu));
        let other = Machine {
            os: Os::Other,
            ..plain
        };
        assert_eq!(Variant::default_for(&other, None), None);
    }

    #[test]
    fn an_older_linux_gets_the_cpu_build_where_the_one_for_its_cards_would_not_start() {
        let nvidia = linux(64, vec![card("RTX 4090", 24)]);
        assert_eq!(
            Variant::default_for(&nvidia, Some("2.35")),
            Some(Variant::Cpu)
        );
        assert_eq!(
            Variant::default_for(&nvidia, Some("2.38")),
            Some(Variant::Cuda12)
        );
        let amd = Machine {
            devices_from: DevicesFrom::AmdSysfs,
            ..linux(64, vec![card("RX 7900", 24)])
        };
        assert_eq!(Variant::default_for(&amd, Some("2.36")), Some(Variant::Cpu));
        assert_eq!(
            Variant::default_for(&apple_silicon(64, 53084), Some("2.17")),
            Some(Variant::Metal)
        );
    }

    #[test]
    fn only_llama_cpp_s_own_linux_builds_are_blocked_by_an_older_glibc() {
        let cpu = PINNED
            .asset(Os::Linux, "x86_64", Variant::Cpu)
            .expect("pinned");
        assert_eq!(cpu.publisher, Publisher::Hedos);
        assert_eq!(cpu.blocked(Some("2.28")), None);
        let vulkan = PINNED
            .asset(Os::Linux, "aarch64", Variant::Vulkan)
            .expect("pinned");
        let blocked = vulkan.blocked(Some("2.35")).expect("blocked");
        assert!(blocked.contains("2.38"), "{blocked}");
        assert_eq!(vulkan.blocked(Some("2.39")), None);
        assert_eq!(vulkan.blocked(None), None);
        let metal = PINNED
            .asset(Os::Macos, "aarch64", Variant::Metal)
            .expect("pinned");
        assert_eq!(metal.blocked(Some("2.17")), None);
        assert_eq!(
            PINNED.variants_starting(Os::Linux, "x86_64", Some("2.35")),
            [Variant::Cpu]
        );
        assert_eq!(
            PINNED.variants_starting(Os::Linux, "x86_64", Some("2.39")),
            PINNED.variants_starting(Os::Linux, "x86_64", None)
        );
    }

    #[test]
    fn the_pin_has_one_build_per_machine_and_variant_and_each_sums_its_runtime() {
        for asset in PINNED.assets {
            assert_eq!(
                PINNED.asset(asset.os, asset.arch, asset.variant),
                Some(asset)
            );
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.file.contains(PINNED.build), "{}", asset.file);
            if let Some(runtime) = asset.cudart {
                assert_eq!(asset.download_bytes(), asset.bytes + runtime.bytes);
                assert!(runtime.file.contains(PINNED.build));
            }
        }
        assert_eq!(PINNED.build_number(), Some(11669));
        assert_eq!(parse_build_tag("b11669"), Some(11669));
        for odd in ["b+1", "b", "11669", "b1a", " b1"] {
            assert_eq!(parse_build_tag(odd), None, "{odd}");
        }
        assert!(PINNED.build_number() >= Some(DECISION_MODELS_BUILD));
        assert_eq!(
            PINNED.url(Publisher::LlamaCpp, "x.tar.gz"),
            "https://github.com/ggml-org/llama.cpp/releases/download/b11669/x.tar.gz"
        );
        assert_eq!(
            PINNED.url(Publisher::Hedos, "x.tar.gz"),
            "https://github.com/theiskaa/hedos/releases/download/llama.cpp-b11669/x.tar.gz"
        );
        // hedos builds the Linux CPU builds and nothing else.
        for asset in PINNED.assets {
            let hedos = asset.os == Os::Linux && asset.variant == Variant::Cpu;
            assert_eq!(asset.publisher == Publisher::Hedos, hedos, "{}", asset.file);
        }
        assert_eq!(
            PINNED.variants_starting(Os::Linux, "x86_64", None),
            [
                Variant::Cpu,
                Variant::Vulkan,
                Variant::Cuda12,
                Variant::Rocm
            ]
        );
    }

    #[test]
    fn hedos_carries_metal_on_apple_silicon_and_the_cpu_build_elsewhere() {
        assert_eq!(bundled_variant(Os::Macos, "aarch64"), Some(Variant::Metal));
        assert_eq!(bundled_variant(Os::Macos, "x86_64"), Some(Variant::Cpu));
        assert_eq!(bundled_variant(Os::Linux, "x86_64"), Some(Variant::Cpu));
        assert_eq!(bundled_variant(Os::Linux, "aarch64"), Some(Variant::Cpu));
        assert_eq!(bundled_variant(Os::Other, "x86_64"), None);
        for (os, arch) in [
            (Os::Macos, "aarch64"),
            (Os::Macos, "x86_64"),
            (Os::Linux, "x86_64"),
            (Os::Linux, "aarch64"),
        ] {
            let variant = bundled_variant(os, arch).unwrap();
            assert!(PINNED.asset(os, arch, variant).is_some(), "{arch}");
        }
    }

    #[test]
    fn a_record_from_before_bundling_was_installed_on_request() {
        let old = r#"{"version":"0.7.0","build":"b11669","variant":"metal","sha256":"0","installed_at":1}"#;
        let record: Installed = serde_json::from_str(old).unwrap();
        assert_eq!(record.source, InstallSource::Requested);
        assert!(record.files.is_empty());
        let bundled = Installed {
            source: InstallSource::Bundled,
            ..record
        };
        let round: Installed =
            serde_json::from_str(&serde_json::to_string(&bundled).unwrap()).unwrap();
        assert_eq!(round.source, InstallSource::Bundled);
    }

    #[test]
    fn a_variant_reads_back_from_its_name() {
        for variant in Variant::ALL {
            assert_eq!(Variant::parse(variant.as_str()), Some(variant));
        }
        assert_eq!(Variant::parse("CUDA-12"), Some(Variant::Cuda12));
        assert_eq!(Variant::parse("opencl"), None);
        assert_eq!(
            install_dir_name("b11669", Variant::Metal, InstallSource::Requested),
            "b11669-metal"
        );
        assert_eq!(
            install_dir_name("b11669", Variant::Cpu, InstallSource::Bundled),
            "b11669-cpu-bundled"
        );
    }

    #[test]
    fn a_glibc_older_than_the_published_builds_need_is_named() {
        assert_eq!(parse_glibc("2.35"), Some((2, 35)));
        assert_eq!(parse_glibc("2.41\n"), Some((2, 41)));
        assert_eq!(parse_glibc("musl"), None);
        let blocked = published_build_blocked("2.35").expect("blocked");
        assert!(
            blocked.contains("2.38") && blocked.contains("2.35"),
            "{blocked}"
        );
        assert_eq!(published_build_blocked("2.38"), None);
        assert_eq!(published_build_blocked("2.39"), None);
        assert_eq!(published_build_blocked("3.0"), None);
    }

    #[test]
    fn a_server_the_loader_could_not_start_says_what_it_lacks() {
        let gomp = "/x/llama-server: error while loading shared libraries: libgomp.so.1: cannot open shared object file: No such file or directory";
        assert_eq!(
            loader_failure(gomp).as_deref(),
            Some(
                "it needs libgomp.so.1, which this system lacks: install libgomp1 on Debian and Ubuntu, libgomp on Fedora"
            )
        );
        let other = "llama-server: error while loading shared libraries: libfoo.so.2: cannot open";
        assert_eq!(
            loader_failure(other).as_deref(),
            Some("it needs libfoo.so.2, which this system lacks")
        );
        let glibc = "./llama-server: /lib/aarch64-linux-gnu/libc.so.6: version `GLIBC_2.38' not found (required by ./libggml-base.so.0)";
        assert_eq!(
            loader_failure(glibc).as_deref(),
            Some("it needs glibc 2.38 or newer, which this system lacks")
        );
        assert_eq!(loader_failure("version: 0.7.0 (build 11669)"), None);
    }
}
