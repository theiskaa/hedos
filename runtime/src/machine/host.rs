//! Where the probe's readings come from: the real machine, or a test's.

use std::collections::HashMap;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kernel::install::InstallProviderId;
use kernel::machine::{Cores, FreeDisk, Os};

use crate::governor::GovernorConfig;

/// How long a tool may take to describe the machine. llama.cpp lists its
/// devices in about 70 ms; a tool still going after this is wedged.
const RUN_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the Ollama daemon may take to accept a connection on loopback.
const OLLAMA_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
/// The most of a tool's output read: a device list is a few lines.
const OUTPUT_CAP: u64 = 64 * 1024;
/// How long a tool's output may take to arrive after it exits, before a
/// process it left holding the pipe is given up on.
const OUTPUT_GRACE: Duration = Duration::from_millis(200);

/// The readings a machine probe takes. Each is optional where the machine may
/// not have it to give.
pub trait Host {
    /// The operating system.
    fn os(&self) -> Os;
    /// The CPU architecture.
    fn arch(&self) -> String;
    /// The processor's name.
    fn chip(&self) -> Option<String>;
    /// The processor's cores.
    fn cores(&self) -> Cores;
    /// Total system memory, as the governor reads it.
    fn memory_bytes(&self) -> u64;
    /// Free space where each provider's pulls land.
    fn free_disk(&self) -> Vec<FreeDisk>;
    /// Metal's recommended working set, on a GPU sharing system memory.
    fn metal_working_set(&self) -> Option<u64>;
    /// `program` on PATH.
    fn find(&self, program: &str) -> Option<PathBuf>;
    /// What `program` prints on stdout when it exits 0 in time.
    fn run(&self, program: &Path, args: &[&str]) -> Option<String>;
    /// The contents of `path`.
    fn read(&self, path: &Path) -> Option<String>;
    /// The `/sys/class/drm/cardN` directories.
    fn drm_cards(&self) -> Vec<PathBuf>;
    /// Whether Ollama is installed or its daemon answers.
    fn ollama(&self) -> bool;
    /// Whether `uv` is installed.
    fn uv(&self) -> bool;
}

/// The machine this process runs on.
pub struct SystemHost;

impl Host for SystemHost {
    fn os(&self) -> Os {
        Os::current()
    }

    fn arch(&self) -> String {
        std::env::consts::ARCH.to_owned()
    }

    fn chip(&self) -> Option<String> {
        crate::chip::name()
    }

    fn cores(&self) -> Cores {
        let logical = std::thread::available_parallelism()
            .map_or(1, |count| u32::try_from(count.get()).unwrap_or(u32::MAX));
        #[cfg(target_os = "macos")]
        {
            let read = |name: &std::ffi::CStr| {
                crate::sys::u64_value(name)
                    .and_then(|count| u32::try_from(count).ok())
                    .filter(|count| *count > 0)
            };
            Cores {
                performance: read(c"hw.perflevel0.physicalcpu"),
                efficiency: read(c"hw.perflevel1.physicalcpu"),
                logical,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Cores {
                performance: None,
                efficiency: None,
                logical,
            }
        }
    }

    fn memory_bytes(&self) -> u64 {
        let mebibytes = GovernorConfig::detect().total_memory_mb.max(1);
        u64::try_from(mebibytes).unwrap_or(1) * 1024 * 1024
    }

    fn free_disk(&self) -> Vec<FreeDisk> {
        let environment: HashMap<String, String> = std::env::vars().collect();
        let home = environment
            .get("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        [
            (
                InstallProviderId::ollama(),
                crate::install::ollama::models_root(&environment),
            ),
            (
                InstallProviderId::huggingface(),
                crate::boot::hf_cache_root(&home),
            ),
        ]
        .into_iter()
        .filter_map(|(provider, root)| free_at(&root).map(|bytes| FreeDisk { provider, bytes }))
        .collect()
    }

    fn metal_working_set(&self) -> Option<u64> {
        #[cfg(target_os = "macos")]
        {
            crate::metal::recommended_working_set()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    fn find(&self, program: &str) -> Option<PathBuf> {
        kernel::fs::find_on_path(program)
    }

    fn run(&self, program: &Path, args: &[&str]) -> Option<String> {
        run_bounded(program, args, RUN_TIMEOUT)
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn drm_cards(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
            return Vec::new();
        };
        let mut cards: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                // `card0` is a GPU; `card0-HDMI-A-1` is one of its outputs.
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("card"))
                    .is_some_and(|index| {
                        !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())
                    })
            })
            .map(|entry| entry.path())
            .collect();
        cards.sort();
        cards
    }

    fn ollama(&self) -> bool {
        let environment: HashMap<String, String> = std::env::vars().collect();
        crate::install::ollama::daemon_binary(&environment).is_some() || ollama_answers()
    }

    fn uv(&self) -> bool {
        crate::environment::uv_binary().is_some()
    }
}

/// Whether something accepts connections where the Ollama daemon listens: a
/// daemon running from a container or another install hedos cannot find the
/// binary of still serves.
fn ollama_answers() -> bool {
    let address = crate::install::ollama::DEFAULT_BASE_URL.trim_start_matches("http://");
    address
        .parse::<SocketAddr>()
        .is_ok_and(|address| TcpStream::connect_timeout(&address, OLLAMA_CONNECT_TIMEOUT).is_ok())
}

/// The free space on the disk `path` is on, read at its nearest existing
/// ancestor, since a store that has never been pulled into may not exist yet.
fn free_at(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|ancestor| ancestor.exists())?;
    fs2::available_space(existing).ok()
}

/// Run `program` with `args` and no stdin, and return its stdout if it exits 0
/// within `timeout`. A program still running then is killed.
///
/// Its output is read on a thread of its own, so a full pipe never stalls it,
/// and handed over through a channel with a deadline rather than joined: a
/// process the program started can hold the pipe open past the program's own
/// end, and is left to it rather than waited on.
fn run_bounded(program: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (sender, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.take(OUTPUT_CAP).read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let output = output.recv_timeout(OUTPUT_GRACE).ok()?;
    status
        .filter(ExitStatus::success)
        .and_then(|_| String::from_utf8(output).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_succeeds_gives_its_stdout() {
        let output = run_bounded(Path::new("/bin/echo"), &["hello"], RUN_TIMEOUT);
        assert_eq!(output.as_deref(), Some("hello\n"));
    }

    #[test]
    fn a_program_that_fails_gives_nothing() {
        assert!(
            run_bounded(
                Path::new("/bin/sh"),
                &["-c", "echo no; exit 1"],
                RUN_TIMEOUT
            )
            .is_none()
        );
    }

    #[test]
    fn a_missing_program_gives_nothing() {
        assert!(run_bounded(Path::new("/nonexistent/llama-server"), &[], RUN_TIMEOUT).is_none());
    }

    #[test]
    fn a_process_left_holding_the_pipe_does_not_hold_up_the_answer() {
        let started = Instant::now();
        let output = run_bounded(
            Path::new("/bin/sh"),
            &["-c", "sleep 10 & exit 0"],
            Duration::from_millis(500),
        );
        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_wrapper_past_its_time_is_given_up_on_whatever_it_started() {
        let started = Instant::now();
        let output = run_bounded(
            Path::new("/bin/sh"),
            &["-c", "sleep 10; true"],
            Duration::from_millis(100),
        );
        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn free_space_is_read_where_the_path_would_be() {
        let missing = std::env::temp_dir().join("hedos-no-such-store/models");
        assert!(free_at(&missing).is_some());
    }

    #[test]
    fn a_program_past_its_time_is_killed_and_gives_nothing() {
        let started = Instant::now();
        let output = run_bounded(
            Path::new("/bin/sh"),
            &["-c", "exec sleep 10"],
            Duration::from_millis(100),
        );
        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
