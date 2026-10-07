//! A reader that closes stdout early (`hedos scan | true`) ends the command
//! quietly instead of a panic, and a command that started a model server
//! stops it rather than leaving it running. The server here is a stand-in
//! `llama-server` on `PATH` that records its pid and streams one token after
//! another until its client goes away.

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);
const PATIENCE: Duration = Duration::from_secs(20);

/// The stand-in `llama-server`: answers `/health`, and streams a chat reply
/// that never ends on `/v1/chat/completions`.
const STAND_IN: &str = r#"#!/bin/sh
echo $$ >> "$HEDOS_TEST_PIDS"
while [ $# -gt 0 ]; do [ "$1" = --port ] && port=$2; shift; done
exec /usr/bin/python3 -I -c '
import http.server, sys, time
class Answer(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"token \"}}]}\n\n"
        try:
            while True:
                self.wfile.write(chunk)
                self.wfile.flush()
                time.sleep(0.02)
        except OSError:
            pass
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Answer).serve_forever()
' "$port"
"#;

struct Home(PathBuf);

impl Home {
    /// A fresh home of its own. Named by a counter, not the clock: the tests
    /// run at once, and two that read the clock in the same microsecond
    /// would share one home, and one's cleanup would end the other's server.
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "hedos-closed-pipe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// A home holding one chat GGUF in a watched folder and the stand-in
    /// `llama-server` on `PATH`, already scanned.
    fn with_a_model() -> Self {
        let home = Self::new();
        for dir in ["gguf", ".config", "bin"] {
            std::fs::create_dir_all(home.0.join(dir)).unwrap();
        }
        std::fs::write(home.0.join("gguf/tiny-chat.gguf"), chat_gguf()).unwrap();
        std::fs::write(
            home.0.join(".config/hedos.toml"),
            format!(
                "[models]\nwatched_folders = [\"{}\"]\n",
                home.0.join("gguf").display()
            ),
        )
        .unwrap();
        let server = home.0.join("bin/llama-server");
        std::fs::write(&server, STAND_IN).unwrap();
        std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let scanned = home
            .hedos(&["scan"])
            .stdout(Stdio::null())
            .output()
            .unwrap();
        assert!(scanned.status.success(), "scan failed: {scanned:?}");
        home
    }

    fn hedos(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hedos"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.0.join("bin").display()),
            )
            .env("HF_HOME", self.0.join("hf"))
            .env("HF_HUB_OFFLINE", "1")
            .env("OLLAMA_HOST", "127.0.0.1:9")
            .env("OLLAMA_MODELS", self.0.join("ollama"))
            .env("HEDOS_TEST_PIDS", self.pids_file())
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    fn pids_file(&self) -> PathBuf {
        self.0.join("server-pids")
    }

    fn server_pids(&self) -> Vec<String> {
        std::fs::read_to_string(self.pids_file())
            .unwrap_or_default()
            .lines()
            .map(|pid| pid.trim().to_owned())
            .collect()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        for pid in self.server_pids() {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &pid])
                .stderr(Stdio::null())
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A GGUF header for a llama chat model, with no tensors.
fn chat_gguf() -> Vec<u8> {
    let string = |value: &str| {
        let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(value.as_bytes());
        bytes
    };
    let mut kvs = Vec::new();
    kvs.extend(string("general.architecture"));
    kvs.extend(8u32.to_le_bytes());
    kvs.extend(string("llama"));
    kvs.extend(string("llama.context_length"));
    kvs.extend(4u32.to_le_bytes());
    kvs.extend(2048u32.to_le_bytes());
    kvs.extend(string("tokenizer.chat_template"));
    kvs.extend(8u32.to_le_bytes());
    kvs.extend(string("{{ messages }}"));
    let mut bytes = b"GGUF".to_vec();
    bytes.extend(3u32.to_le_bytes());
    bytes.extend(0u64.to_le_bytes());
    bytes.extend(3u64.to_le_bytes());
    bytes.extend(kvs);
    bytes
}

/// Spawn `command` with stdout piped, then close the reading end at once.
fn spawn_with_stdout_closed(command: &mut Command) -> Child {
    let mut child = command.stdout(Stdio::piped()).spawn().unwrap();
    drop(child.stdout.take());
    child
}

/// The output of `child` once it exits within [`PATIENCE`]; killed and failed
/// otherwise.
fn finished(mut child: Child, what: &str) -> Output {
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > PATIENCE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what} kept running after its stdout closed");
        }
        std::thread::sleep(POLL);
    }
    child.wait_with_output().unwrap()
}

/// Whether the process `pid` is gone within [`PATIENCE`].
fn gone(pid: &str) -> bool {
    let start = Instant::now();
    loop {
        let alive = Command::new("/bin/kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return true;
        }
        if start.elapsed() > PATIENCE {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

fn assert_quiet_success(output: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{what}: {stderr}");
    assert!(
        output.status.success(),
        "{what}: {:?} {stderr}",
        output.status
    );
}

#[test]
fn scan_and_ls_end_quietly_when_stdout_closes() {
    let home = Home::new();
    for args in [&["scan"][..], &["ls"], &["--json", "scan"]] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hedos"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &home.0)
            .env("PATH", "/usr/bin:/bin")
            .env("HF_HOME", home.0.join("hf"))
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        let output = finished(spawn_with_stdout_closed(&mut command), &format!("{args:?}"));
        assert_quiet_success(&output, &format!("{args:?}"));
    }
}

/// Whether the interpreter the stand-in server runs on works here. On a Mac
/// without the Command Line Tools `/usr/bin/python3` is only a stub that
/// offers to install them, so the server cases are skipped there.
fn python_runs() -> bool {
    let runs = Command::new("/usr/bin/python3")
        .args(["-I", "-c", ""])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !runs {
        eprintln!("skipped: /usr/bin/python3 does not run, and the stand-in server needs it");
    }
    runs
}

#[test]
fn a_run_whose_stdout_closes_stops_the_server_it_started() {
    if !python_runs() {
        return;
    }
    let home = Home::with_a_model();
    let run = spawn_with_stdout_closed(&mut home.hedos(&["run", "tiny-chat", "hi"]));
    let output = finished(run, "run");
    assert_quiet_success(&output, "run");
    let servers = home.server_pids();
    assert!(
        !servers.is_empty(),
        "no llama-server was started: {output:?}"
    );
    for server in servers {
        assert!(gone(&server), "llama-server {server} outlived hedos run");
    }
}

#[test]
fn a_chat_whose_stdout_closes_stops_the_server_it_started() {
    if !python_runs() {
        return;
    }
    let home = Home::with_a_model();
    let mut chat = home.hedos(&["chat", "tiny-chat"]);
    chat.stdin(Stdio::piped());
    let mut chat = chat.stdout(Stdio::piped()).spawn().unwrap();
    drop(chat.stdout.take());
    {
        use std::io::Write;
        let mut stdin = chat.stdin.take().unwrap();
        stdin.write_all(b"hi\n").unwrap();
    }
    let output = finished(chat, "chat");
    assert_quiet_success(&output, "chat");
    let servers = home.server_pids();
    assert!(
        !servers.is_empty(),
        "no llama-server was started: {output:?}"
    );
    for server in servers {
        assert!(gone(&server), "llama-server {server} outlived hedos chat");
    }
}
