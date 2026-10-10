//! A termination (SIGTERM, or SIGHUP from a closed terminal) stops `hedos`
//! the way Ctrl-C does, so the llama-server it started is stopped with it
//! rather than left running. Each test runs the built binary against an
//! isolated home whose `llama-server` is a stand-in that records its pid and
//! either never becomes ready, so the signal lands during a cold start, or
//! answers every request, so it lands on a warm, idle server. A closed
//! terminal is a real one: a pseudo-terminal whose controlling side is closed.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);
const PATIENCE: Duration = Duration::from_secs(20);

/// The stand-in `llama-server`: with `HEDOS_TEST_READY` set it answers every
/// request on its `--port` with an empty JSON object, else it never listens.
/// Asked for its devices, as the machine probe asks where Metal says nothing,
/// it lists none and exits without counting as a server.
const STAND_IN: &str = r#"#!/bin/sh
if [ "$1" = --list-devices ]; then
  echo "Available devices:"
  exit 0
fi
echo $$ >> "$HEDOS_TEST_PIDS"
if [ -n "$HEDOS_TEST_READY" ]; then
  while [ $# -gt 0 ]; do [ "$1" = --port ] && port=$2; shift; done
  exec /usr/bin/python3 -I -c '
import http.server, sys
class Answer(http.server.BaseHTTPRequestHandler):
    def answer(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")
    do_GET = answer
    do_POST = answer
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), Answer).serve_forever()
' "$port"
fi
exec /bin/sleep 60
"#;

struct Home {
    root: PathBuf,
}

impl Home {
    /// A home holding one chat GGUF in a watched folder, a settings file
    /// naming it, and the stand-in `llama-server` on `PATH`, already scanned.
    fn new() -> Self {
        Self::with_settings("")
    }

    /// [`Home::new`], its settings file ending with `settings`.
    fn with_settings(settings: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "hedos-termination-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let home = Self { root };
        for dir in ["gguf", ".config", "bin"] {
            std::fs::create_dir_all(home.root.join(dir)).unwrap();
        }
        std::fs::write(home.root.join("gguf/tiny-chat.gguf"), chat_gguf()).unwrap();
        std::fs::write(
            home.root.join(".config/hedos.toml"),
            format!(
                "[models]\nwatched_folders = [\"{}\"]\n{settings}",
                home.root.join("gguf").display()
            ),
        )
        .unwrap();
        let server = home.root.join("bin/llama-server");
        std::fs::write(&server, STAND_IN).unwrap();
        std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let scanned = home.hedos(&["scan"]).output().unwrap();
        assert!(scanned.status.success(), "scan failed: {scanned:?}");
        home
    }

    fn hedos(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hedos"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.root)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("HF_HOME", self.root.join("hf"))
            .env("HF_HUB_OFFLINE", "1")
            .env("OLLAMA_HOST", "127.0.0.1:9")
            .env("OLLAMA_MODELS", self.root.join("ollama"))
            .env("HEDOS_TEST_PIDS", self.pids_file())
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn pids_file(&self) -> PathBuf {
        self.root.join("server-pids")
    }

    /// The pid of the first llama-server started, once one is.
    fn server_pid(&self) -> u32 {
        let pid = poll(|| {
            std::fs::read_to_string(self.pids_file())
                .ok()
                .and_then(|pids| pids.lines().next().and_then(|pid| pid.trim().parse().ok()))
        });
        pid.expect("a llama-server was started")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if let Ok(pids) = std::fs::read_to_string(self.pids_file()) {
            for pid in pids.lines() {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", pid.trim()])
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
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

/// Call `check` every [`POLL`] until it answers, for at most [`PATIENCE`].
fn poll<T>(mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if start.elapsed() > PATIENCE {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

fn signal(child: &Child, name: &str) {
    let sent = Command::new("/bin/kill")
        .args([&format!("-{name}"), &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
}

fn exit_of(child: &mut Child) -> ExitStatus {
    poll(|| child.try_wait().ok().flatten()).expect("hedos exited")
}

/// Whether the process `pid` is gone, exited and reaped, within [`PATIENCE`].
fn gone(pid: u32) -> bool {
    poll(|| gone_now(pid).then_some(())).is_some()
}

fn gone_now(pid: u32) -> bool {
    !Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn reaped_after(signal_name: &str, code: i32) {
    let home = Home::new();
    let mut run = home
        .hedos(&["run", "tiny-chat", "hi"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let server = home.server_pid();

    signal(&run, signal_name);
    assert_eq!(exit_of(&mut run).code(), Some(code));
    assert!(gone(server), "llama-server {server} outlived hedos");
}

#[test]
fn a_terminated_run_stops_the_server_it_started() {
    reaped_after("TERM", 128 + 15);
}

#[test]
fn a_hung_up_run_stops_the_server_it_started() {
    reaped_after("HUP", 128 + 1);
}

#[test]
fn a_terminated_serve_stops_in_order_and_stops_its_servers() {
    let home = Home::new();
    let mut serve = home
        .hedos(&["serve", "-p", "0"])
        .env("HEDOS_TEST_READY", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let address = serve_address(&mut serve);

    // A chat request warms a server; it is idle by the time the answer is read.
    let answer = chat(&address);
    assert!(answer.starts_with("HTTP/1.1"), "{answer}");
    let server = home.server_pid();
    assert!(!gone_now(server), "the warm server stopped on its own");

    signal(&serve, "TERM");
    assert!(exit_of(&mut serve).success());
    assert!(gone(server), "llama-server {server} outlived hedos serve");
}

/// The address `serve` announces on its first line of stdout.
fn serve_address(serve: &mut Child) -> String {
    let mut first_line = String::new();
    let stdout = serve.stdout.take().unwrap();
    BufReader::new(stdout).read_line(&mut first_line).unwrap();
    first_line
        .trim()
        .rsplit("http://")
        .next()
        .and_then(|rest| rest.strip_suffix("/v1"))
        .expect("serve names its address")
        .to_owned()
}

/// A chat request for the home's model to the gateway at `address`, and the
/// whole answer, head included, once the gateway closes it.
fn chat(address: &str) -> String {
    let mut request = TcpStream::connect(address).unwrap();
    let body = r#"{"model":"tiny-chat","messages":[{"role":"user","content":"hi"}]}"#;
    write!(
        request,
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    request.read_to_string(&mut answer).unwrap();
    answer
}

/// Start `hedos` with SIGHUP ignored, as `nohup` starts a command.
fn ignoring_hangups(command: &mut Command) -> &mut Command {
    // SAFETY: the closure runs in the child between fork and exec and calls
    // only `signal`, which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        })
    }
}

#[test]
fn a_serve_started_ignoring_hangups_keeps_ignoring_them() {
    let home = Home::new();
    let mut serve = home.hedos(&["serve", "-p", "0"]);
    let mut serve = ignoring_hangups(serve.stdout(Stdio::piped()))
        .spawn()
        .unwrap();
    let address = serve_address(&mut serve);

    signal(&serve, "HUP");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        serve.try_wait().unwrap().is_none(),
        "serve stopped on a hangup it was started ignoring"
    );
    assert!(TcpStream::connect(&address).is_ok());

    signal(&serve, "TERM");
    assert!(exit_of(&mut serve).success());
}

/// A gateway on a cold start that never ends, with a chat request waiting on
/// it from another thread: the gateway, the request's answer once it comes,
/// and the stand-in server's pid.
fn serving_a_request_in_flight(home: &Home) -> (Child, JoinHandle<String>, u32) {
    let mut serve = home
        .hedos(&["serve", "-p", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let address = serve_address(&mut serve);
    let request = std::thread::spawn(move || chat(&address));
    let server = home.server_pid();
    (serve, request, server)
}

#[test]
fn a_first_ctrl_c_waits_for_the_requests_in_flight_and_a_second_cuts_them() {
    let home = Home::new();
    let (mut serve, request, server) = serving_a_request_in_flight(&home);

    signal(&serve, "INT");
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        serve.try_wait().unwrap().is_none(),
        "a first Ctrl-C did not wait for the request in flight"
    );
    assert!(!request.is_finished());

    signal(&serve, "INT");
    assert!(exit_of(&mut serve).success());
    let answer = request.join().unwrap();
    assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    assert!(answer.contains("the gateway is stopping"), "{answer}");
    assert!(gone(server), "llama-server {server} outlived hedos serve");
}

#[test]
fn a_terminated_serve_cuts_the_requests_in_flight_after_a_grace() {
    let home = Home::new();
    let (mut serve, request, server) = serving_a_request_in_flight(&home);

    let terminated = Instant::now();
    signal(&serve, "TERM");
    assert!(exit_of(&mut serve).success());
    let waited = terminated.elapsed();
    assert!(
        waited >= Duration::from_secs(4) && waited < Duration::from_secs(10),
        "stopped after {waited:?}"
    );
    let answer = request.join().unwrap();
    assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    assert!(gone(server), "llama-server {server} outlived hedos serve");
}

/// A command running on a pseudo-terminal of its own, as its session's
/// controlling terminal. Its output is read as it comes, so it never blocks
/// on a full terminal; closing it is closing the terminal window.
struct Terminal {
    child: Child,
    keys: Option<File>,
    output: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

impl Terminal {
    fn spawn(command: &mut Command, stderr: Option<File>) -> Self {
        let (controller, terminal) = open_pty();
        let size = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `TIOCSWINSZ` reads one `winsize`, which lives for the call.
        unsafe { libc::ioctl(controller.as_raw_fd(), libc::TIOCSWINSZ, &size) };

        command
            .env("TERM", "xterm-256color")
            .stdin(terminal.try_clone().unwrap())
            .stdout(terminal.try_clone().unwrap())
            .stderr(match stderr {
                Some(file) => Stdio::from(file),
                None => Stdio::from(terminal.try_clone().unwrap()),
            });
        // SAFETY: the closure runs in the child between fork and exec and
        // calls only `setsid` and `ioctl`, which are async-signal-safe. The
        // terminal is its stdin by then.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1
                    || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        drop(terminal);

        let output = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let mut controller = controller.try_clone().unwrap();
            let output = Arc::clone(&output);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut buffer = [0u8; 65536];
                while !stop.load(Ordering::Relaxed) {
                    let mut waited = libc::pollfd {
                        fd: controller.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // SAFETY: `poll` reads and writes only the one `pollfd`.
                    if unsafe { libc::poll(&mut waited, 1, 50) } <= 0 {
                        continue;
                    }
                    match controller.read(&mut buffer) {
                        Ok(0) | Err(_) => std::thread::sleep(POLL),
                        Ok(read) => output.lock().unwrap().extend_from_slice(&buffer[..read]),
                    }
                }
            })
        };
        Self {
            child,
            keys: Some(controller),
            output,
            stop,
            reader: Some(reader),
        }
    }

    /// How many times `needle` has been written to the terminal so far.
    fn seen(&self, needle: &[u8]) -> usize {
        let output = self.output.lock().unwrap();
        output
            .windows(needle.len())
            .filter(|window| *window == needle)
            .count()
    }

    fn wait_for(&self, needle: &[u8], times: usize) {
        assert!(
            poll(|| (self.seen(needle) >= times).then_some(())).is_some(),
            "the terminal never showed {:?}",
            String::from_utf8_lossy(needle)
        );
    }

    fn type_keys(&mut self, keys: &[u8]) {
        let controller = self.keys.as_mut().unwrap();
        controller.write_all(keys).unwrap();
        controller.flush().unwrap();
    }

    /// Close the terminal, as closing its window does.
    fn close(&mut self) {
        self.keys = None;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.close();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A new pseudo-terminal: its controlling side and the terminal itself. Both
/// are opened close-on-exec, as std opens every file, so no child another
/// test spawns meanwhile inherits either and keeps this terminal open.
fn open_pty() -> (File, File) {
    let open = |path: &str| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(path)
            .unwrap()
    };
    let controller = open("/dev/ptmx");
    let fd = controller.as_raw_fd();
    // SAFETY: both act only on the descriptor they are given.
    let unlocked = unsafe { libc::grantpt(fd) == 0 && libc::unlockpt(fd) == 0 };
    assert!(unlocked, "{}", std::io::Error::last_os_error());
    let terminal = open(&terminal_path(fd));
    (controller, terminal)
}

/// The path of the terminal whose controlling side is `controller`.
fn terminal_path(controller: RawFd) -> String {
    // `ptsname` names it in one buffer shared by every caller, so the tests,
    // which run on several threads, take turns and copy the name out.
    static NAMING: Mutex<()> = Mutex::new(());
    let _turn = NAMING.lock().unwrap();
    // SAFETY: `ptsname` acts only on the descriptor it is given and returns a
    // NUL-terminated path, or null on failure.
    let name = unsafe { libc::ptsname(controller) };
    assert!(!name.is_null(), "{}", std::io::Error::last_os_error());
    // SAFETY: not null, so a NUL-terminated path that no other call changes
    // while the turn is held.
    let path = unsafe { std::ffi::CStr::from_ptr(name) };
    path.to_str().unwrap().to_owned()
}

/// The alternate screen being entered: the shelf drawing itself.
const SHELF_SHOWN: &[u8] = b"\x1b[?1049h";

/// What `serve` writes once it listens, ahead of its address.
const LISTENING: &[u8] = b"gateway listening on http://";

/// The address the `n`th gateway on `terminal` announced, once it has.
fn announced(terminal: &Terminal, n: usize) -> String {
    terminal.wait_for(LISTENING, n);
    let output = terminal.output.lock().unwrap();
    let text = String::from_utf8_lossy(&output).into_owned();
    text.split("gateway listening on http://")
        .nth(n)
        .and_then(|rest| rest.split("/v1").next())
        .expect("serve names its address")
        .to_owned()
}

/// The shelf, on a terminal, after it served the gateway, warmed a server
/// through it, and came back: the terminal and the warm server's pid.
fn a_shelf_with_a_warm_server(home: &Home, stderr: Option<File>) -> (Terminal, u32) {
    let mut command = home.hedos(&["shelf"]);
    command.env("HEDOS_TEST_READY", "1");
    let mut terminal = Terminal::spawn(&mut command, stderr);
    terminal.wait_for(SHELF_SHOWN, 1);
    // The screen drops what was typed before its reader started, so `S` is
    // typed again until the gateway it opens answers.
    let start = Instant::now();
    while terminal.seen(LISTENING) == 0 {
        assert!(
            start.elapsed() < PATIENCE,
            "the shelf never served the gateway"
        );
        terminal.type_keys(b"S");
        let typed = Instant::now();
        while terminal.seen(LISTENING) == 0 && typed.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(POLL);
        }
    }
    let address = announced(&terminal, 1);
    assert!(chat(&address).starts_with("HTTP/1.1"));
    let server = home.server_pid();
    terminal.type_keys(b"\x03");
    terminal.wait_for(SHELF_SHOWN, 2);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !gone_now(server),
        "the warm server stopped with the gateway"
    );
    (terminal, server)
}

/// Close the shelf's terminal and check it ends on its own, cleanly, and
/// stops the server it kept warm.
fn closed_under_the_shelf(stderr_to_file: bool) {
    let home = Home::with_settings("[gateway]\nport = 0\n");
    let log = home.root.join("stderr.log");
    let stderr = stderr_to_file.then(|| File::create(&log).unwrap());
    let (mut terminal, server) = a_shelf_with_a_warm_server(&home, stderr);

    terminal.close();
    let status = exit_of(&mut terminal.child);
    assert!(status.success(), "the shelf ended with {status:?}");
    assert!(gone(server), "llama-server {server} outlived the shelf");
    if stderr_to_file {
        let written = std::fs::read_to_string(&log).unwrap();
        assert!(!written.contains("panicked"), "{written}");
    }
}

#[test]
fn a_shelf_whose_terminal_closes_stops_its_servers() {
    closed_under_the_shelf(false);
}

#[test]
fn a_shelf_whose_terminal_closes_with_stderr_elsewhere_stops_its_servers() {
    closed_under_the_shelf(true);
}

#[test]
fn a_serve_whose_terminal_closes_exits_cleanly() {
    let home = Home::new();
    let mut command = home.hedos(&["serve", "-p", "0"]);
    command.env("HEDOS_TEST_READY", "1");
    let mut terminal = Terminal::spawn(&mut command, None);
    let address = announced(&terminal, 1);
    assert!(chat(&address).starts_with("HTTP/1.1"));
    let server = home.server_pid();

    terminal.close();
    let status = exit_of(&mut terminal.child);
    assert!(status.success(), "serve ended with {status:?}");
    assert!(gone(server), "llama-server {server} outlived hedos serve");
}
