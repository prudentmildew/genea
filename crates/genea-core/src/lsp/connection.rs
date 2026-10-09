//! One language server process and the JSON-RPC traffic with it (LSP's
//! base protocol: `Content-Length`-framed JSON over stdin and stdout).
//!
//! Nothing here runs on the main thread except queueing. A [`Connection`]
//! owns four threads:
//!
//! - the **starter**, which spawns the process through the host and then
//!   becomes the **reader**: it parses what the server writes, answers the
//!   server's own requests at once, drops chatter (`window/logMessage`,
//!   `$/progress`), and queues the rest as [`Event`]s;
//! - the **writer**, which serialises and writes queued messages. A
//!   document's full text is turned into JSON here, off the main thread, and
//!   a `didChange` still waiting to be written is replaced by a newer one,
//!   so a slow server never makes typing wait or queue up copies;
//! - a thread that drains stderr, keeping the last lines for crash notices.
//!
//! Events reach the main thread in batches: the first event after a batch
//! was taken posts an Apply (holding a busy token), and every event until it
//! runs joins it. Each request holds a busy token until its answer has been
//! applied (or the process is gone), so `settle` waits for answers.

use std::{
    collections::{HashMap, VecDeque},
    io::{self, BufRead, BufReader, Write},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use genea_host::{Exit, ProcessControl, ProcessSpec, SharedHost};
use ropey::Rope;
use serde_json::{Value, json};

use crate::{
    jobs::{Busy, Jobs},
    workbench::Core,
};

/// How long a server may take to exit after `shutdown` before it is killed,
/// on the host clock.
pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Stderr lines kept for the crash notice.
const STDERR_LINES: usize = 20;

/// What the server sent, for the main thread.
#[derive(Debug)]
pub(crate) enum Event {
    /// The answer to a request: its `result`, or its `error`.
    Response { id: i64, result: Result<Value, ResponseError> },
    /// A notification, or a request the reader already answered (its
    /// params; e.g. `client/registerCapability`).
    Message { method: String, params: Value },
    /// The process is gone (or never started): how it ended, and why.
    Exited { reason: String },
}

/// An error answer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResponseError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Value,
}

/// Runs an event on the main thread; it finds its server by itself.
pub(crate) type Deliver = Arc<dyn Fn(&mut Core, Event) + Send + Sync>;

/// A message waiting for the writer.
enum Out {
    Json(Value),
    /// `textDocument/didOpen` with the text, serialised by the writer.
    Open { uri: String, language: &'static str, version: i32, text: Rope },
    /// `textDocument/didChange` with the whole text.
    Change { uri: String, version: i32, text: Rope },
}

pub(crate) struct Connection {
    shared: Arc<Shared>,
    jobs: Jobs,
    host: SharedHost,
    next_id: i64,
    stopped: bool,
}

struct Shared {
    outgoing: Mutex<Outgoing>,
    /// Signals the writer.
    ready: Condvar,
    /// What `settle` waits for.
    waits: Mutex<Waits>,
    /// Taken by the reader once the process's output ends (it waits for the
    /// exit itself), so a kill never waits on it.
    control: Mutex<Option<Box<dyn ProcessControl>>>,
    incoming: Mutex<Incoming>,
    stderr: Mutex<VecDeque<String>>,
    /// What `workspace/workspaceFolders` answers.
    folders: Value,
    jobs: Jobs,
    deliver: Deliver,
}

#[derive(Default)]
struct Waits {
    /// The process is gone: nothing is waited for any more.
    exited: bool,
    /// Busy tokens of requests waiting for an answer, by id.
    pending: HashMap<i64, Busy>,
    /// Set by `stop`: the shutdown request's id.
    shutdown: Option<i64>,
    /// Set by `stop`: held until the process has exited.
    stopping: Option<Busy>,
}

#[derive(Default)]
struct Outgoing {
    queue: VecDeque<Out>,
    /// No more messages: the writer closes stdin once the queue is empty.
    closed: bool,
}

#[derive(Default)]
struct Incoming {
    events: Vec<(Event, Option<Busy>)>,
    /// A batch is posted and hasn't run yet: new events join it.
    scheduled: bool,
}

impl Connection {
    /// Starts `spec` in the background. Messages can be queued at once; they
    /// are written once the process runs. `folders` is the workspace folder
    /// list the server is told about.
    pub(crate) fn start(host: SharedHost, spec: ProcessSpec, folders: Value, jobs: &Jobs, deliver: Deliver) -> Self {
        let shared = Arc::new(Shared {
            outgoing: Mutex::default(),
            ready: Condvar::new(),
            waits: Mutex::default(),
            control: Mutex::default(),
            incoming: Mutex::default(),
            stderr: Mutex::default(),
            folders,
            jobs: jobs.clone(),
            deliver,
        });
        let (thread_shared, thread_host) = (shared.clone(), host.clone());
        let name = format!("genea: language server {}", spec.program_name().to_string_lossy());
        std::thread::Builder::new()
            .name(name)
            .spawn(move || run(thread_host, spec, thread_shared))
            .expect("spawn a language server thread");
        Connection { shared, jobs: jobs.clone(), host, next_id: 0, stopped: false }
    }

    /// Sends a request; its answer comes back as [`Event::Response`] with
    /// the returned id. `settle` waits for it.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut waits = self.shared.waits.lock().unwrap();
        if !waits.exited {
            waits.pending.insert(id, self.jobs.busy());
        }
        drop(waits);
        self.send(Out::Json(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })));
        id
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.send(Out::Json(json!({ "jsonrpc": "2.0", "method": method, "params": params })));
    }

    /// `textDocument/didOpen`; the text becomes JSON on the writer thread.
    pub(crate) fn open(&self, uri: String, language: &'static str, version: i32, text: Rope) {
        self.send(Out::Open { uri, language, version, text });
    }

    /// `textDocument/didChange` with the whole text. If the last queued
    /// message is a change of the same document, it is replaced: only the
    /// newest text matters.
    pub(crate) fn change(&self, uri: String, version: i32, text: Rope) {
        let mut outgoing = self.shared.outgoing.lock().unwrap();
        if let Some(Out::Change { uri: last, .. }) = outgoing.queue.back()
            && *last == uri
        {
            outgoing.queue.pop_back();
        }
        outgoing.queue.push_back(Out::Change { uri, version, text });
        self.shared.ready.notify_one();
    }

    /// Stops waiting for an answer: `settle` no longer waits for it. A late
    /// answer still arrives.
    pub(crate) fn forget(&self, id: i64) {
        let busy = self.shared.waits.lock().unwrap().pending.remove(&id);
        drop(busy);
    }

    /// The last lines the server wrote to stderr.
    #[allow(dead_code)] // For a log view; crash notices read them from `Event::Exited`.
    pub(crate) fn stderr(&self) -> Vec<String> {
        self.shared.stderr.lock().unwrap().iter().cloned().collect()
    }

    /// Asks the server to shut down and exit (`shutdown`, then `exit` once
    /// it answers), and kills it if it hasn't exited within
    /// [`STOP_TIMEOUT`]. Its events stop mattering, and `settle` waits until
    /// it has exited.
    pub(crate) fn stop(&mut self) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        let mut waits = self.shared.waits.lock().unwrap();
        if !waits.exited {
            waits.shutdown = Some(id);
            waits.stopping = Some(self.jobs.busy());
        }
        drop(waits);
        self.send(Out::Json(json!({ "jsonrpc": "2.0", "id": id, "method": "shutdown" })));
        let shared = self.shared.clone();
        self.host.clock().after(STOP_TIMEOUT, Box::new(move || shared.kill()));
    }

    fn send(&self, out: Out) {
        let mut outgoing = self.shared.outgoing.lock().unwrap();
        outgoing.queue.push_back(out);
        self.shared.ready.notify_one();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shared {
    /// Kills the process (if it is still running) and stops writing.
    fn kill(&self) {
        if let Some(control) = self.control.lock().unwrap().as_mut() {
            let _ = control.kill();
        }
        self.close();
    }

    /// No more messages: the writer closes stdin once the queue is empty.
    fn close(&self) {
        self.outgoing.lock().unwrap().closed = true;
        self.ready.notify_one();
    }

    /// Queues an event for the main thread (posting a batch if none is
    /// waiting). `busy` is a request's token, finished when its answer has
    /// been applied.
    fn push(self: &Arc<Self>, event: Event, busy: Option<Busy>) {
        let mut incoming = self.incoming.lock().unwrap();
        incoming.events.push((event, busy));
        if !std::mem::replace(&mut incoming.scheduled, true) {
            let shared = self.clone();
            self.jobs.busy().finish(Box::new(move |core| {
                let events = {
                    let mut incoming = shared.incoming.lock().unwrap();
                    incoming.scheduled = false;
                    std::mem::take(&mut incoming.events)
                };
                for (event, busy) in events {
                    (shared.deliver)(core, event);
                    drop(busy);
                }
            }));
        }
    }

    fn write_now(&self, message: Value) {
        let mut outgoing = self.outgoing.lock().unwrap();
        outgoing.queue.push_front(Out::Json(message));
        self.ready.notify_one();
    }
}

/// The starter and reader thread.
fn run(host: SharedHost, spec: ProcessSpec, shared: Arc<Shared>) {
    let mut child = match host.processes().spawn(&spec) {
        Ok(child) => child,
        Err(error) => {
            let reason = format!("{} didn't start: {error}", spec.program.display());
            return finish(&shared, reason);
        }
    };
    *shared.control.lock().unwrap() = Some(child.control);
    if let Some(stdin) = child.stdin.take() {
        let writer_shared = shared.clone();
        std::thread::Builder::new()
            .name("genea: language server writer".into())
            .spawn(move || write_loop(stdin, &writer_shared))
            .expect("spawn a language server thread");
    }
    if let Some(stderr) = child.stderr.take() {
        let stderr_shared = shared.clone();
        std::thread::Builder::new()
            .name("genea: language server stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    let mut lines = stderr_shared.stderr.lock().unwrap();
                    if lines.len() == STDERR_LINES {
                        lines.pop_front();
                    }
                    lines.push_back(line);
                }
            })
            .expect("spawn a language server thread");
    }
    if let Some(stdout) = child.stdout.take() {
        let mut stdout = BufReader::new(stdout);
        while let Ok(Some(message)) = read_message(&mut stdout) {
            receive(&shared, message);
        }
    }
    // The output ended: the process exited or is about to. Kill it in case
    // it only closed stdout, then collect its exit.
    shared.close();
    let control = shared.control.lock().unwrap().take();
    let exit = control.and_then(|mut control| {
        let _ = control.kill();
        control.wait().ok()
    });
    let reason = match exit {
        Some(Exit { code: Some(code), .. }) => format!("it exited with code {code}"),
        Some(Exit { signal: Some(signal), .. }) => format!("it was stopped by signal {signal}"),
        _ => "it stopped".to_owned(),
    };
    let last = shared.stderr.lock().unwrap().iter().rev().find(|line| !line.trim().is_empty()).cloned();
    let reason = match last {
        Some(line) => format!("{reason}: {}", line.trim()),
        None => reason,
    };
    finish(&shared, reason);
}

/// Reports the exit and lets go of everything `settle` waits for.
fn finish(shared: &Arc<Shared>, reason: String) {
    shared.close();
    shared.push(Event::Exited { reason }, None);
    let released = {
        let mut waits = shared.waits.lock().unwrap();
        waits.exited = true;
        (std::mem::take(&mut waits.pending), waits.stopping.take())
    };
    drop(released);
}

/// Handles one message from the server.
fn receive(shared: &Arc<Shared>, message: Value) {
    let id = message.get("id").cloned();
    match message.get("method").and_then(Value::as_str).map(str::to_owned) {
        // A response to one of Genea's requests.
        None => {
            let Some(id) = id.and_then(|id| id.as_i64()) else { return };
            let (shutdown, busy) = {
                let mut waits = shared.waits.lock().unwrap();
                (waits.shutdown == Some(id), waits.pending.remove(&id))
            };
            {
                if shutdown {
                    // Answered `shutdown`: say `exit`, and close stdin after it.
                    let mut outgoing = shared.outgoing.lock().unwrap();
                    outgoing.queue.push_back(Out::Json(json!({ "jsonrpc": "2.0", "method": "exit" })));
                    outgoing.closed = true;
                    shared.ready.notify_one();
                    return;
                }
            }
            let result = match message.get("error") {
                Some(error) => Err(ResponseError {
                    code: error["code"].as_i64().unwrap_or(0),
                    message: error["message"].as_str().unwrap_or_default().to_owned(),
                    data: error.get("data").cloned().unwrap_or(Value::Null),
                }),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            shared.push(Event::Response { id, result }, busy);
        }
        // A request from the server: answer it now, then tell the main
        // thread what it said.
        Some(method) if id.is_some() => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let answer = match method.as_str() {
                "workspace/configuration" => {
                    let items = params["items"].as_array().map_or(0, Vec::len);
                    Ok(Value::Array(vec![Value::Null; items]))
                }
                "workspace/workspaceFolders" => Ok(shared.folders.clone()),
                "client/registerCapability"
                | "client/unregisterCapability"
                | "window/workDoneProgress/create"
                | "window/showMessageRequest"
                | "workspace/diagnostic/refresh"
                | "workspace/semanticTokens/refresh"
                | "workspace/inlayHint/refresh"
                | "workspace/codeLens/refresh"
                | "workspace/foldingRange/refresh"
                | "workspace/inlineValue/refresh" => Ok(Value::Null),
                _ => Err(json!({ "code": -32601, "message": format!("Genea doesn't handle {method}") })),
            };
            let known = answer.is_ok();
            let reply = match answer {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
            };
            // Queue the event before answering, so that once the server has
            // the answer, `settle` waits for the main thread to take it.
            if known {
                shared.push(Event::Message { method, params }, None);
            }
            shared.write_now(reply);
        }
        // A notification.
        Some(method) => {
            if matches!(method.as_str(), "window/logMessage" | "$/progress" | "telemetry/event" | "$/logTrace") {
                return;
            }
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            shared.push(Event::Message { method, params }, None);
        }
    }
}

/// The writer thread: writes queued messages until the queue is closed and
/// empty, then closes stdin.
fn write_loop(mut stdin: Box<dyn Write + Send>, shared: &Shared) {
    loop {
        let out = {
            let mut outgoing = shared.outgoing.lock().unwrap();
            loop {
                if let Some(out) = outgoing.queue.pop_front() {
                    break Some(out);
                }
                if outgoing.closed {
                    break None;
                }
                outgoing = shared.ready.wait(outgoing).unwrap();
            }
        };
        let Some(out) = out else { break };
        let message = match out {
            Out::Json(message) => message,
            Out::Open { uri, language, version, text } => json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": { "uri": uri, "languageId": language, "version": version, "text": text.to_string() } },
            }),
            Out::Change { uri, version, text } => json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": { "textDocument": { "uri": uri, "version": version }, "contentChanges": [{ "text": text.to_string() }] },
            }),
        };
        let body = message.to_string();
        let written = write!(stdin, "Content-Length: {}\r\n\r\n", body.len())
            .and_then(|()| stdin.write_all(body.as_bytes()))
            .and_then(|()| stdin.flush());
        if written.is_err() {
            break;
        }
    }
    shared.close();
    // Dropping stdin closes it.
}

/// Reads one `Content-Length`-framed message. `None` at the end of the
/// output; an error for a broken frame.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.expect("checked above");
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    // A body that isn't JSON is skipped, not fatal.
    Ok(Some(serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages_until_the_end() {
        let input = b"Content-Length: 2\r\n\r\n{}Content-Type: x\r\ncontent-length: 7\r\n\r\n{\"a\":1}";
        let mut reader = BufReader::new(&input[..]);
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({})));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({ "a": 1 })));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }
}
