//! Live Unix domain control socket server (`--ctl [<SOCK>]`) and `cadence ctl` CLI client.
//!
//! Server design:
//! - Binds a `UnixListener` with `0o600` permissions (owner-only), checking whether an
//!   existing socket has an active listener before removing any stale socket file.
//! - Accepts newline-delimited JSON [`HostRequest`]s on a background thread and forwards
//!   them over a bounded `sync_channel` (`32` slots) into the live GUI loop, firing the
//!   GUI's wake callback immediately so requests respond with near-zero latency even
//!   while the window is idle or occluded.
//! - `listen --live` runs off-GUI on a short-lived background thread draining
//!   `AudioRuntime`'s lock-free stereo tap ring so live Jam/MIDI/GPU rendering never stalls.
//! - Cleans up both the socket file and any auto-generated `ear.wav` on drop/exit.
use super::protocol::{
    execute_host_request_bytes, read_bounded_line, read_request_line, HostRequest,
    HostRequestContext, HostRequestOutcome, RequestLine, MAX_REQUEST_BYTES,
};
use super::OutcomeHooks;
use anyhow::{bail, ensure, Context, Result};
use cadence_app::App;
use cadence_engine::audio::AudioRuntime;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Summary of `--ctl` requests serviced during one host poll cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CtlPollSummary {
    pub handled: usize,
    pub mutated: bool,
    pub quit: bool,
}

/// Parse a CLI bar range for `cadence ctl listen --bars`:
/// - Half-open `A..B` (e.g. `1..5` -> bars 1, 2, 3, 4 -> `(1, 5)`)
/// - Inclusive `A..=B` (e.g. `1..=4` -> bars 1, 2, 3, 4 -> `(1, 5)`)
pub fn parse_listen_bars(spec: &str) -> Result<(u32, u32)> {
    let trimmed = spec.trim();
    if let Some((a, b)) = trimmed.split_once("..=") {
        let start: u32 = a
            .trim()
            .parse()
            .with_context(|| format!("Invalid start bar in '{spec}'"))?;
        let end_inc: u32 = b
            .trim()
            .parse()
            .with_context(|| format!("Invalid end bar in '{spec}'"))?;
        ensure!(
            start >= 1 && end_inc >= start,
            "Invalid inclusive bar range '{spec}': expected START..=END with 1 <= START <= END"
        );
        let end = end_inc
            .checked_add(1)
            .context("Bar range end overflowed u32")?;
        return Ok((start, end));
    }
    if let Some((a, b)) = trimmed.split_once("..") {
        let start: u32 = a
            .trim()
            .parse()
            .with_context(|| format!("Invalid start bar in '{spec}'"))?;
        let end: u32 = b
            .trim()
            .parse()
            .with_context(|| format!("Invalid end bar in '{spec}'"))?;
        ensure!(
            start >= 1 && end > start,
            "Invalid bar range '{spec}': expected 1-based half-open START..END (e.g. 1..5)"
        );
        return Ok((start, end));
    }
    bail!("Invalid --bars range '{spec}': expected START..END (e.g. 1..5) or START..=END (e.g. 1..=4)")
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::fs;
    use std::io::{BufReader, ErrorKind, Write};
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Mutex,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, SystemTime};

    const CTL_QUEUE_CAPACITY: usize = 32;
    const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(40);
    const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(15);
    /// Upper bound on a single `--ctl` JSON response line (16 MiB — accommodates full
    /// non-compact `ctl state` project snapshots up to `MAX_DOCUMENT_BYTES` without OOM).
    const MAX_CTL_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

    struct PendingCtlRequest {
        bytes: Vec<u8>,
        reply_tx: SyncSender<serde_json::Value>,
    }

    type WakeSlot = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

    /// Live Unix socket server wired into the GUI event loop (or test harness).
    pub struct CtlServer {
        path: PathBuf,
        ear_path: PathBuf,
        rx: Receiver<PendingCtlRequest>,
        shutdown: Arc<AtomicBool>,
        wake: WakeSlot,
        live_tap_busy: Arc<AtomicBool>,
        accept_thread: Option<JoinHandle<()>>,
    }

    impl CtlServer {
        /// Bind a `0o600` Unix control socket at `sock_path` with default `ear_path`.
        /// Safely detects live listeners before unlinking any stale socket file.
        pub fn bind(sock_path: PathBuf, ear_path: PathBuf) -> Result<Self> {
            if let Some(parent) = sock_path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Creating control socket directory {}", parent.display())
                })?;
            }
            if let Ok(meta) = fs::symlink_metadata(&sock_path) {
                if !meta.file_type().is_socket() {
                    bail!(
                        "Refusing to overwrite non-socket file at {}",
                        sock_path.display()
                    );
                }
                match UnixStream::connect(&sock_path) {
                    Ok(_) => {
                        bail!(
                            "Control socket {} is already active in another Cadence instance",
                            sock_path.display()
                        );
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            ErrorKind::ConnectionRefused | ErrorKind::NotFound
                        ) =>
                    {
                        fs::remove_file(&sock_path).with_context(|| {
                            format!("Removing stale control socket {}", sock_path.display())
                        })?;
                    }
                    Err(err) => {
                        return Err(err).with_context(|| {
                            format!("Probing existing control socket {}", sock_path.display())
                        });
                    }
                }
            }

            let listener = UnixListener::bind(&sock_path).with_context(|| {
                use std::os::unix::ffi::OsStrExt;
                let byte_len = sock_path.as_os_str().as_bytes().len();
                if byte_len > cadence_core::store::SessionDir::MAX_UNIX_SOCKET_PATH_BYTES {
                    format!(
                        "Binding control socket {} ({byte_len} bytes exceeds the Unix ~104-byte SUN_LEN limit; pass a shorter path such as /tmp/cadence.sock or omit the path to let --ctl pick a short socket automatically)",
                        sock_path.display()
                    )
                } else {
                    format!("Binding control socket {}", sock_path.display())
                }
            })?;
            if let Err(err) = fs::set_permissions(&sock_path, fs::Permissions::from_mode(0o600)) {
                let _ = fs::remove_file(&sock_path);
                return Err(err).with_context(|| {
                    format!(
                        "Setting 0600 permissions on control socket {}",
                        sock_path.display()
                    )
                });
            }
            listener
                .set_nonblocking(true)
                .context("Configuring nonblocking control socket listener")?;

            let (tx, rx) = mpsc::sync_channel::<PendingCtlRequest>(CTL_QUEUE_CAPACITY);
            let shutdown = Arc::new(AtomicBool::new(false));
            let wake: WakeSlot = Arc::new(Mutex::new(None));
            let live_tap_busy = Arc::new(AtomicBool::new(false));

            let accept_shutdown = Arc::clone(&shutdown);
            let accept_wake = Arc::clone(&wake);
            let accept_thread = thread::Builder::new()
                .name("cadence-ctl-accept".into())
                .spawn(move || {
                    while !accept_shutdown.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let conn_tx = tx.clone();
                                let conn_wake = Arc::clone(&accept_wake);
                                let conn_shutdown = Arc::clone(&accept_shutdown);
                                let mut err_stream = stream.try_clone().ok();
                                if let Err(err) = thread::Builder::new()
                                    .name("cadence-ctl-conn".into())
                                    .spawn(move || {
                                        handle_client_stream(
                                            stream,
                                            conn_tx,
                                            conn_wake,
                                            conn_shutdown,
                                        );
                                    })
                                {
                                    eprintln!(
                                        "cadence: warning: failed to spawn control connection handler: {err}"
                                    );
                                    if let Some(mut s) = err_stream.take() {
                                        let resp = serde_json::json!({
                                            "ok": false,
                                            "error": format!(
                                                "Failed to spawn control connection handler: {err}"
                                            ),
                                        });
                                        let _ = write_json_line(&mut s, &resp);
                                    }
                                }
                            }
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                thread::sleep(ACCEPT_POLL_INTERVAL);
                            }
                            Err(_) => {
                                if accept_shutdown.load(Ordering::Acquire) {
                                    break;
                                }
                                thread::sleep(ACCEPT_POLL_INTERVAL);
                            }
                        }
                    }
                })
                .context("Spawning control socket listener thread")?;

            Ok(Self {
                path: sock_path,
                ear_path,
                rx,
                shutdown,
                wake,
                live_tap_busy,
                accept_thread: Some(accept_thread),
            })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Register an immediate wakeup callback (e.g., winit `EventLoopProxy`).
        pub fn set_wake(&self, cb: Arc<dyn Fn() + Send + Sync>) {
            if let Ok(mut slot) = self.wake.lock() {
                *slot = Some(cb);
            }
        }

        /// Synchronously service all queued `--ctl` requests on the GUI/host thread.
        pub fn poll_and_dispatch(
            &self,
            app: &mut App,
            audio: &mut AudioRuntime,
            hooks: &mut dyn OutcomeHooks,
        ) -> CtlPollSummary {
            let mut summary = CtlPollSummary::default();
            while let Ok(pending) = self.rx.try_recv() {
                summary.handled += 1;
                let mut ctx = HostRequestContext {
                    audio: Some(&mut *audio),
                    default_ear_path: self.ear_path.clone(),
                    live_tap_busy: Some(&self.live_tap_busy),
                };
                match execute_host_request_bytes(app, &pending.bytes, &mut ctx, hooks) {
                    HostRequestOutcome::Immediate {
                        response,
                        mutated,
                        quit,
                        ..
                    } => {
                        summary.mutated |= mutated;
                        summary.quit |= quit;
                        let _ = pending.reply_tx.send(response);
                    }
                    HostRequestOutcome::DeferredLiveListen(job) => {
                        let req_id = job.request_id().cloned();
                        let worker_tx = pending.reply_tx.clone();
                        let spawned = thread::Builder::new()
                            .name("cadence-live-listen".into())
                            .spawn(move || {
                                let (response, _) = job.execute();
                                let _ = worker_tx.send(response);
                            });
                        if let Err(err) = spawned {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "revision": app.revision(),
                                "error": format!("Failed to spawn live listen worker: {err}"),
                            });
                            if let Some(id) = req_id {
                                payload["id"] = id;
                            }
                            let _ = pending.reply_tx.send(payload);
                        }
                    }
                }
            }
            summary
        }
    }

    impl Drop for CtlServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Release);
            if let Some(handle) = self.accept_thread.take() {
                let _ = handle.join();
            }
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(&self.ear_path);
        }
    }

    fn handle_client_stream(
        stream: UnixStream,
        tx: SyncSender<PendingCtlRequest>,
        wake: WakeSlot,
        shutdown: Arc<AtomicBool>,
    ) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(CLIENT_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(CLIENT_IO_TIMEOUT));
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut line = Vec::with_capacity(MAX_REQUEST_BYTES);
        while !shutdown.load(Ordering::Acquire) {
            match read_request_line(&mut reader, &mut line) {
                Ok(RequestLine::Eof) => break,
                Ok(RequestLine::Oversized) => {
                    let resp = serde_json::json!({
                        "ok": false,
                        "error": "JSONL request exceeds 64 KiB",
                    });
                    if write_json_line(&mut writer, &resp).is_err() {
                        break;
                    }
                }
                Ok(RequestLine::Complete) => {
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let (reply_tx, reply_rx) = mpsc::sync_channel::<serde_json::Value>(1);
                    let req = PendingCtlRequest {
                        bytes: line.clone(),
                        reply_tx,
                    };
                    if tx.try_send(req).is_err() {
                        let resp = serde_json::json!({
                            "ok": false,
                            "error": "Control socket queue is full or server shutting down",
                        });
                        let _ = write_json_line(&mut writer, &resp);
                        break;
                    }
                    if let Ok(slot) = wake.lock() {
                        if let Some(cb) = slot.as_ref() {
                            cb();
                        }
                    }
                    match reply_rx.recv_timeout(CLIENT_IO_TIMEOUT) {
                        Ok(resp) => {
                            if write_json_line(&mut writer, &resp).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            let resp = serde_json::json!({
                                "ok": false,
                                "error": "Timed out waiting for Cadence host to process control request",
                            });
                            let _ = write_json_line(&mut writer, &resp);
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }

    fn write_json_line(writer: &mut UnixStream, value: &serde_json::Value) -> Result<()> {
        serde_json::to_writer(&mut *writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    }

    /// Resolve the Unix control socket path from explicit `--sock`, `--session-dir`,
    /// `CADENCE_CTL_SOCK`, or live sockets under Cadence's platform sessions directory.
    pub fn resolve_ctl_socket(
        explicit_sock: Option<&Path>,
        explicit_session_dir: Option<&Path>,
    ) -> Result<PathBuf> {
        if let Some(path) = explicit_sock {
            return Ok(path.to_path_buf());
        }
        if let Some(dir) = explicit_session_dir {
            return Ok(cadence_core::store::SessionDir::at(dir.to_path_buf()).ctl_socket_path());
        }
        if let Ok(env_sock) = std::env::var("CADENCE_CTL_SOCK") {
            let trimmed = env_sock.trim();
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed));
            }
        }
        if let Ok(sessions_dir) = cadence_core::store::SessionDir::sessions_dir() {
            const MAX_CTL_SESSION_SCAN: usize = 256;
            if let Ok(entries) = fs::read_dir(&sessions_dir) {
                let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
                for entry in entries.take(MAX_CTL_SESSION_SCAN).flatten() {
                    let sock = cadence_core::store::SessionDir::at(entry.path()).ctl_socket_path();
                    if let Ok(meta) = fs::symlink_metadata(&sock) {
                        if meta.file_type().is_socket() {
                            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                            candidates.push((modified, sock));
                        }
                    }
                }
                candidates.sort_by_key(|a| std::cmp::Reverse(a.0));
                for (_, sock) in &candidates {
                    if UnixStream::connect(sock).is_ok() {
                        return Ok(sock.clone());
                    }
                }
                if let Some((_, first)) = candidates.into_iter().next() {
                    return Ok(first);
                }
            }
        }
        bail!(
            "No active Cadence control socket found. Start Cadence with --ctl, pass --sock PATH, or set CADENCE_CTL_SOCK."
        )
    }

    /// Send one [`HostRequest`] to a running Cadence `--ctl` Unix socket and return its parsed JSON response.
    pub fn send_ctl_request(sock_path: &Path, request: &HostRequest) -> Result<serde_json::Value> {
        send_ctl_request_with_limit(sock_path, request, MAX_CTL_RESPONSE_BYTES)
    }

    pub(crate) fn send_ctl_request_with_limit(
        sock_path: &Path,
        request: &HostRequest,
        max_response_bytes: usize,
    ) -> Result<serde_json::Value> {
        let mut payload = serde_json::to_vec(request)?;
        ensure!(
            payload.len() <= MAX_REQUEST_BYTES,
            "Control request exceeds {} byte limit",
            MAX_REQUEST_BYTES
        );
        payload.push(b'\n');

        let mut stream = UnixStream::connect(sock_path).with_context(|| {
            format!(
                "Connecting to Cadence control socket {}",
                sock_path.display()
            )
        })?;
        stream.set_read_timeout(Some(CLIENT_IO_TIMEOUT))?;
        stream.set_write_timeout(Some(CLIENT_IO_TIMEOUT))?;

        stream
            .write_all(&payload)
            .context("Writing control request")?;
        stream.flush().context("Flushing control request")?;

        let mut reader = BufReader::new(stream);
        let mut raw_line = Vec::with_capacity(4096.min(max_response_bytes));
        match read_bounded_line(&mut reader, &mut raw_line, max_response_bytes, false)
            .context("Reading control response")?
        {
            RequestLine::Eof => {
                bail!("Cadence control socket closed without sending a response")
            }
            RequestLine::Oversized => {
                bail!("Control socket response exceeds {max_response_bytes} byte limit")
            }
            RequestLine::Complete => {
                let text = std::str::from_utf8(&raw_line)
                    .context("Control socket response is not valid UTF-8")?;
                let value: serde_json::Value = serde_json::from_str(text.trim())
                    .context("Parsing control socket JSON response")?;
                Ok(value)
            }
        }
    }
}

#[cfg(unix)]
pub use unix_impl::{resolve_ctl_socket, send_ctl_request, CtlServer};

#[cfg(not(unix))]
mod non_unix_impl {
    use super::*;

    pub struct CtlServer;

    impl CtlServer {
        pub fn bind(_sock_path: PathBuf, _ear_path: PathBuf) -> Result<Self> {
            bail!("Live Unix control socket (--ctl) is only supported on Unix platforms")
        }

        pub fn path(&self) -> &Path {
            Path::new("")
        }

        pub fn set_wake(&self, _cb: Arc<dyn Fn() + Send + Sync>) {}

        pub fn poll_and_dispatch(
            &self,
            _app: &mut App,
            _audio: &mut AudioRuntime,
            _hooks: &mut dyn OutcomeHooks,
        ) -> CtlPollSummary {
            CtlPollSummary::default()
        }
    }

    pub fn resolve_ctl_socket(
        _explicit_sock: Option<&Path>,
        _explicit_session_dir: Option<&Path>,
    ) -> Result<PathBuf> {
        bail!("Live Unix control socket (`cadence ctl`) is only supported on Unix platforms")
    }

    pub fn send_ctl_request(
        _sock_path: &Path,
        _request: &HostRequest,
    ) -> Result<serde_json::Value> {
        bail!("Live Unix control socket (`cadence ctl`) is only supported on Unix platforms")
    }
}

#[cfg(not(unix))]
pub use non_unix_impl::{resolve_ctl_socket, send_ctl_request, CtlServer};

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    #[test]
    fn send_ctl_request_bounds_response_lines_and_rejects_oversized_requests() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("mock-ctl.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        // 1. Oversized outgoing request (> 64 KiB) is rejected before even connecting.
        let huge_req = HostRequest {
            cmd: Some("x".repeat(MAX_REQUEST_BYTES + 64)),
            ..HostRequest::default()
        };
        let req_err = send_ctl_request(&sock_path, &huge_req).unwrap_err();
        assert!(
            req_err.to_string().contains("exceeds"),
            "unexpected oversized request error: {req_err:#}"
        );

        // 2. Oversized response without even a trailing newline is rejected promptly by the cap.
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let _ = stream.write_all(&vec![b'a'; 8192]);
            let _ = stream.flush();
        });
        let small_req = HostRequest {
            meters: true,
            ..HostRequest::default()
        };
        let resp_err =
            unix_impl::send_ctl_request_with_limit(&sock_path, &small_req, 1024).unwrap_err();
        assert!(
            resp_err.to_string().contains("exceeds 1024 byte limit"),
            "unexpected oversized response error: {resp_err:#}"
        );
        server.join().unwrap();
    }
}
