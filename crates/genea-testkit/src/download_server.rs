//! The local download fixture server: a real HTTP server on 127.0.0.1.
//!
//! Tests publish files under the URLs Genea really fetches
//! (`https://nodejs.org/dist/…`), and the test host forwards every fetch it
//! has no scripted answer for to this server, over real HTTP. So downloads
//! stream through a real socket with a real `Content-Length`, and never
//! touch the network.
//!
//! [`hold`](DownloadServer::hold) stops a response halfway through its body
//! until [`release`](DownloadServer::release), so a test can look at Genea
//! while a download is in flight.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// See the module docs. Dropping it stops the server and releases holds.
pub struct DownloadServer {
    state: Arc<State>,
    addr: SocketAddr,
}

#[derive(Default)]
struct State {
    files: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    holds: Mutex<Holds>,
    changed: Condvar,
    requests: Mutex<Vec<String>>,
    stopped: AtomicBool,
}

#[derive(Default)]
struct Holds {
    /// URLs whose responses stop halfway.
    held: HashSet<String>,
    /// How many responses are waiting at the halfway point, per URL.
    waiting: HashMap<String, usize>,
}

/// How long `wait_held` waits before failing the test.
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

impl DownloadServer {
    /// Starts a server on an ephemeral loopback port.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the download fixture server");
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let accept_state = state.clone();
        std::thread::Builder::new()
            .name("download fixture server".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if accept_state.stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let state = accept_state.clone();
                    let _ = std::thread::Builder::new()
                        .name("download fixture response".into())
                        .spawn(move || serve(&state, stream));
                }
            })
            .expect("spawn the download fixture server");
        DownloadServer { state, addr }
    }

    /// Serves `body` for `url` (the URL Genea fetches, e.g.
    /// `https://nodejs.org/dist/index.json`), replacing what was there.
    pub fn publish(&self, url: impl Into<String>, body: impl Into<Vec<u8>>) {
        self.state.files.lock().unwrap().insert(url.into(), Arc::new(body.into()));
    }

    /// Stops serving `url`; it answers 404 again.
    pub fn unpublish(&self, url: &str) {
        self.state.files.lock().unwrap().remove(url);
    }

    /// The body published for `url`, if any.
    pub fn published(&self, url: &str) -> Option<Vec<u8>> {
        self.state.files.lock().unwrap().get(url).map(|b| b.to_vec())
    }

    /// Makes responses for `url` stop after half their body until
    /// [`release`](Self::release).
    pub fn hold(&self, url: impl Into<String>) {
        self.state.holds.lock().unwrap().held.insert(url.into());
    }

    /// Lets held responses for `url` finish, and stops holding new ones.
    pub fn release(&self, url: &str) {
        self.state.holds.lock().unwrap().held.remove(url);
        self.state.changed.notify_all();
    }

    /// Blocks until a response for `url` has sent half its body and is
    /// waiting to be released. Panics after a timeout.
    pub fn wait_held(&self, url: &str) {
        let holds = self.state.holds.lock().unwrap();
        let (_holds, timeout) = self
            .state
            .changed
            .wait_timeout_while(holds, WAIT_TIMEOUT, |h| h.waiting.get(url).copied().unwrap_or(0) == 0)
            .unwrap();
        assert!(!timeout.timed_out(), "no response for {url} reached its hold");
    }

    /// Every URL requested so far, in order, published or not.
    pub fn requests(&self) -> Vec<String> {
        self.state.requests.lock().unwrap().clone()
    }

    /// How many times `url` was requested.
    pub fn request_count(&self, url: &str) -> usize {
        self.state.requests.lock().unwrap().iter().filter(|r| *r == url).count()
    }

    /// The loopback URL the server answers `url` at.
    pub fn local_url(&self, url: &str) -> String {
        format!("http://{}/{}", self.addr, url.replacen("://", "/", 1))
    }
}

impl Drop for DownloadServer {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::SeqCst);
        self.state.holds.lock().unwrap().held.clear();
        self.state.changed.notify_all();
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
    }
}

/// Answers one request, then closes the connection.
fn serve(state: &State, mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the fixture stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Skip the headers.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => {}
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let url = original_url(path);
    state.requests.lock().unwrap().push(url.clone());
    let body = state.files.lock().unwrap().get(&url).cloned();
    let Some(body) = body else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let half = body.len() / 2;
    if stream.write_all(&body[..half]).and_then(|()| stream.flush()).is_err() {
        return;
    }
    wait_while_held(state, &url);
    let _ = stream.write_all(&body[half..]);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn wait_while_held(state: &State, url: &str) {
    let mut holds = state.holds.lock().unwrap();
    if !holds.held.contains(url) {
        return;
    }
    *holds.waiting.entry(url.to_owned()).or_default() += 1;
    state.changed.notify_all();
    let mut holds = state.changed.wait_while(holds, |h| h.held.contains(url)).unwrap();
    *holds.waiting.get_mut(url).unwrap() -= 1;
}

/// `/https/nodejs.org/dist/index.json` → `https://nodejs.org/dist/index.json`.
fn original_url(path: &str) -> String {
    let path = path.trim_start_matches('/');
    match path.split_once('/') {
        Some((scheme, rest)) => format!("{scheme}://{rest}"),
        None => path.to_owned(),
    }
}
