//! The **fake LSP server** (spec #19, Testing Decisions; ticket #42): a
//! small language server speaking JSON-RPC over stdio, scripted per test to
//! reply, delay, crash or flood. It stands in for tsgo (and later
//! `oxlint --lsp` and `oxfmt --lsp`).
//!
//! It runs two ways, with the same code:
//!
//! - **In a core test**, as a scripted process of the [`TestHost`]: build a
//!   [`FakeLsp`], [`install`](FakeLsp::install) it under the program name
//!   Genea starts (`tsc`), and ask it afterwards what reached it
//!   ([`received`](FakeLsp::received), [`wait_for`](FakeLsp::wait_for)).
//! - **As a binary**, `genea-fake-lsp`, for the benchmark harness, which
//!   drives the real Genea: copy it to where Genea looks for tsgo and put its
//!   [`LspScript`] (as JSON, [`LspScript::to_json`]) beside it, named like
//!   the binary plus `.json` (or name the file in `GENEA_FAKE_LSP_SCRIPT`).
//!
//! What it does, unless the script says otherwise: answers `initialize`
//! (choosing UTF-8 positions when offered), keeps the text of open
//! documents, answers `textDocument/diagnostic` with one diagnostic per
//! occurrence of each scripted marker in the document, answers `shutdown`,
//! and exits on `exit` or when its input closes.

use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{FakeProcess, TestHost};

/// How long [`FakeLsp::wait_for`] waits before failing the test. Real
/// time: it only guards against hangs.
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// What a fake server does. Plain data, so the binary can read it as JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LspScript {
    /// Each occurrence of a marker's text in an open document is reported
    /// as a diagnostic covering it.
    pub markers: Vec<Marker>,
    /// Exits with code 1, without answering, on receiving this method.
    pub crash_on: Option<String>,
    /// Never answers or sends anything.
    pub silent: bool,
    /// Waits this long (real time) before every answer.
    pub delay: Duration,
    /// Sends this many `window/logMessage` notifications right after
    /// answering `initialize`.
    pub flood: usize,
    /// Glob patterns registered for `workspace/didChangeWatchedFiles`
    /// (`client/registerCapability`) once the client says `initialized`.
    pub watch: Vec<String>,
}

/// Text the fake reports wherever it occurs.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub text: String,
    /// The LSP severity: 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u32,
    pub message: String,
    /// The diagnostic's code, like a TypeScript error number.
    pub code: Option<i64>,
}

impl LspScript {
    pub fn to_json(&self) -> String {
        let markers: Vec<Value> = self
            .markers
            .iter()
            .map(|m| json!({ "text": m.text, "severity": m.severity, "message": m.message, "code": m.code }))
            .collect();
        json!({
            "markers": markers,
            "crashOn": self.crash_on,
            "silent": self.silent,
            "delayMs": self.delay.as_millis() as u64,
            "flood": self.flood,
            "watch": self.watch,
        })
        .to_string()
    }

    /// Reads a script written by [`to_json`](Self::to_json). Missing keys
    /// take their defaults.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let markers = value["markers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| Marker {
                text: m["text"].as_str().unwrap_or_default().to_owned(),
                severity: m["severity"].as_u64().unwrap_or(1) as u32,
                message: m["message"].as_str().unwrap_or_default().to_owned(),
                code: m["code"].as_i64(),
            })
            .collect();
        Ok(LspScript {
            markers,
            crash_on: value["crashOn"].as_str().map(str::to_owned),
            silent: value["silent"].as_bool().unwrap_or(false),
            delay: Duration::from_millis(value["delayMs"].as_u64().unwrap_or(0)),
            flood: value["flood"].as_u64().unwrap_or(0) as usize,
            watch: value["watch"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
        })
    }
}

/// A message that reached a fake server: a request or notification the
/// client sent, or the client's answer to one of the fake's own requests
/// (under that request's method).
#[derive(Clone, Debug, PartialEq)]
pub struct Received {
    pub method: String,
    /// The params, or the answer's `result`.
    pub params: Value,
}

/// A scripted fake language server for core tests. A cheap handle: clone
/// it, install it on the test host, and keep a clone to change its script
/// and look at what reached it. Every process started from it shares one
/// log.
#[derive(Clone, Default)]
pub struct FakeLsp {
    script: Arc<Mutex<LspScript>>,
    log: Arc<Log>,
}

#[derive(Default)]
struct Log {
    received: Mutex<Vec<Received>>,
    changed: Condvar,
    starts: AtomicUsize,
}

impl FakeLsp {
    /// A well-behaved server that reports nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reports an error wherever `text` occurs in an open document.
    pub fn error(self, text: &str, message: &str) -> Self {
        self.marker(text, 1, message, None)
    }

    /// Reports a warning wherever `text` occurs in an open document.
    pub fn warning(self, text: &str, message: &str) -> Self {
        self.marker(text, 2, message, None)
    }

    /// Reports a diagnostic with any LSP severity and code wherever `text`
    /// occurs.
    pub fn marker(self, text: &str, severity: u32, message: &str, code: Option<i64>) -> Self {
        let marker = Marker { text: text.into(), severity, message: message.into(), code };
        self.script.lock().unwrap().markers.push(marker);
        self
    }

    /// Exits with code 1, without answering, on receiving `method`
    /// (`initialize` makes every start crash).
    pub fn crash_on(self, method: &str) -> Self {
        self.set_crash_on(Some(method));
        self
    }

    /// Changes the method it crashes on, from the next message on; `None`
    /// stops crashing.
    pub fn set_crash_on(&self, method: Option<&str>) {
        self.script.lock().unwrap().crash_on = method.map(str::to_owned);
    }

    /// Never answers or sends anything.
    pub fn silent(self) -> Self {
        self.script.lock().unwrap().silent = true;
        self
    }

    /// Waits this long (real time) before every answer.
    pub fn delay(self, delay: Duration) -> Self {
        self.script.lock().unwrap().delay = delay;
        self
    }

    /// Sends `count` `window/logMessage` notifications right after
    /// answering `initialize`.
    pub fn flood(self, count: usize) -> Self {
        self.script.lock().unwrap().flood = count;
        self
    }

    /// Registers this glob for `workspace/didChangeWatchedFiles` once
    /// initialized, as tsgo does for the files it depends on.
    pub fn watch(self, glob: &str) -> Self {
        self.script.lock().unwrap().watch.push(glob.into());
        self
    }

    /// The script, e.g. to write it beside the binary.
    pub fn script(&self) -> LspScript {
        self.script.lock().unwrap().clone()
    }

    /// Plays every start of `program` (matched on its file name, `tsc` for
    /// tsgo) on the test host.
    pub fn install(&self, host: &TestHost, program: &str) {
        let fake = self.clone();
        host.processes().script(program, move |_spec, io| fake.run(io));
    }

    fn run(&self, io: FakeProcess) -> i32 {
        self.log.starts.fetch_add(1, Ordering::SeqCst);
        let FakeProcess { stdin, stdout, .. } = &io;
        let log = &self.log;
        serve(&self.script, stdin, stdout, |received| {
            log.received.lock().unwrap().push(received);
            log.changed.notify_all();
        })
    }

    /// How many times a process of this fake has started.
    pub fn starts(&self) -> usize {
        self.log.starts.load(Ordering::SeqCst)
    }

    /// The params of every `method` message received so far, in order.
    pub fn received(&self, method: &str) -> Vec<Value> {
        let received = self.log.received.lock().unwrap();
        received.iter().filter(|r| r.method == method).map(|r| r.params.clone()).collect()
    }

    /// Every message received so far, in order.
    pub fn all_received(&self) -> Vec<Received> {
        self.log.received.lock().unwrap().clone()
    }

    /// Waits until at least `count` `method` messages have arrived, and
    /// returns them all. Messages travel through pipes and threads, so a
    /// test that checks what Genea *sent* waits here; it fails after 10 s.
    pub fn wait_for(&self, method: &str, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let mut received = self.log.received.lock().unwrap();
        loop {
            let found: Vec<Value> =
                received.iter().filter(|r| r.method == method).map(|r| r.params.clone()).collect();
            if found.len() >= count {
                return found;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!("the fake language server got {} `{method}` messages, not {count}", found.len());
            }
            received = self.log.changed.wait_timeout(received, left).unwrap().0;
        }
    }
}

/// Plays a fake language server on `input` and `output` until `exit`, the
/// end of the input, or a scripted crash. Returns the exit code. Every
/// message that arrives goes to `record` first.
pub fn serve(script: &Mutex<LspScript>, input: impl Read, output: impl Write, mut record: impl FnMut(Received)) -> i32 {
    let mut input = BufReader::new(input);
    let mut out = Output { writer: output, next_id: 0, requests: HashMap::new() };
    let mut documents: HashMap<String, String> = HashMap::new();
    let mut utf8 = false;
    loop {
        let message = match read_message(&mut input) {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => return 0,
        };
        // The script is read per message, so a test can change it mid-run.
        let script = &script.lock().unwrap().clone();
        let method = message["method"].as_str().map(str::to_owned);
        let Some(method) = method else {
            // The client's answer to one of the fake's requests.
            let request = message["id"].as_str().and_then(|id| out.requests.remove(id));
            let method = request.unwrap_or_else(|| "(unknown response)".into());
            record(Received { method, params: message["result"].clone() });
            continue;
        };
        let params = message["params"].clone();
        record(Received { method: method.clone(), params: params.clone() });
        if script.silent {
            continue;
        }
        if script.crash_on.as_deref() == Some(method.as_str()) {
            return 1;
        }
        let id = message.get("id").cloned();
        let answer = |out: &mut Output<_>, result: Value| {
            if let Some(id) = &id {
                std::thread::sleep(script.delay);
                out.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
        };
        match method.as_str() {
            "initialize" => {
                let offered = params["capabilities"]["general"]["positionEncodings"].as_array();
                utf8 = offered.is_some_and(|kinds| kinds.iter().any(|k| k == "utf-8"));
                let capabilities = json!({
                    "positionEncoding": if utf8 { "utf-8" } else { "utf-16" },
                    "textDocumentSync": { "openClose": true, "change": 1 },
                    "diagnosticProvider": { "identifier": "fake", "interFileDependencies": true, "workspaceDiagnostics": false },
                });
                answer(&mut out, json!({ "capabilities": capabilities, "serverInfo": { "name": "fake-lsp", "version": "7.0.0-fake" } }));
                for i in 0..script.flood {
                    out.send(&json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 4, "message": format!("flood {i}") } }));
                }
            }
            "initialized" if !script.watch.is_empty() => {
                let watchers: Vec<Value> = script.watch.iter().map(|glob| json!({ "globPattern": glob })).collect();
                let registration =
                    json!({ "id": "watchers", "method": "workspace/didChangeWatchedFiles", "registerOptions": { "watchers": watchers } });
                out.request("client/registerCapability", json!({ "registrations": [registration] }));
            }
            "textDocument/didOpen" => {
                let document = &params["textDocument"];
                let uri = document["uri"].as_str().unwrap_or_default().to_owned();
                documents.insert(uri, document["text"].as_str().unwrap_or_default().to_owned());
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
                if let Some(text) = documents.get_mut(uri) {
                    for change in params["contentChanges"].as_array().into_iter().flatten() {
                        apply_change(text, change, utf8);
                    }
                }
            }
            "textDocument/didClose" => {
                documents.remove(params["textDocument"]["uri"].as_str().unwrap_or_default());
            }
            "textDocument/diagnostic" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
                let text = documents.get(uri).map(String::as_str).unwrap_or_default();
                answer(&mut out, json!({ "kind": "full", "items": diagnostics(script, text, utf8) }));
            }
            "shutdown" => answer(&mut out, Value::Null),
            "exit" => return 0,
            _ if id.is_some() => {
                let error = json!({ "code": -32601, "message": format!("fake-lsp doesn't handle {method}") });
                std::thread::sleep(script.delay);
                out.send(&json!({ "jsonrpc": "2.0", "id": id, "error": error }));
            }
            _ => {}
        }
    }
}

struct Output<W> {
    writer: W,
    next_id: u64,
    /// The fake's own requests waiting for the client: id → method.
    requests: HashMap<String, String>,
}

impl<W: Write> Output<W> {
    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        let _ = write!(self.writer, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = self.writer.flush();
    }

    fn request(&mut self, method: &str, params: Value) {
        self.next_id += 1;
        let id = format!("fake-{}", self.next_id);
        self.requests.insert(id.clone(), method.to_owned());
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }
}

/// Reads one `Content-Length`-framed message. `None` at the end of input.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Content-Length"))?;
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(io::Error::other)
}

/// A diagnostic for every occurrence of every marker in `text`.
fn diagnostics(script: &LspScript, text: &str, utf8: bool) -> Vec<Value> {
    let mut items = Vec::new();
    for marker in &script.markers {
        if marker.text.is_empty() {
            continue;
        }
        for (start, _) in text.match_indices(&marker.text) {
            let range = json!({
                "start": position(text, start, utf8),
                "end": position(text, start + marker.text.len(), utf8),
            });
            let mut item = json!({ "range": range, "severity": marker.severity, "message": marker.message, "source": "fake" });
            if let Some(code) = marker.code {
                item["code"] = json!(code);
            }
            items.push(item);
        }
    }
    items
}

/// An LSP position of a byte offset: the line, and the column in UTF-8
/// bytes or UTF-16 units.
fn position(text: &str, offset: usize, utf8: bool) -> Value {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = &text[line_start..offset];
    let character = if utf8 { column.len() } else { column.encode_utf16().count() };
    json!({ "line": line, "character": character })
}

/// The byte offset of an LSP position in `text`.
fn offset(text: &str, position: &Value, utf8: bool) -> usize {
    let line = position["line"].as_u64().unwrap_or(0) as usize;
    let character = position["character"].as_u64().unwrap_or(0) as usize;
    let line_start = if line == 0 {
        0
    } else {
        text.match_indices('\n').nth(line - 1).map_or(text.len(), |(i, _)| i + 1)
    };
    let rest = &text[line_start..];
    let line_text = &rest[..rest.find('\n').unwrap_or(rest.len())];
    let mut units = 0;
    for (i, c) in line_text.char_indices() {
        if units >= character {
            return line_start + i;
        }
        units += if utf8 { c.len_utf8() } else { c.len_utf16() };
    }
    line_start + line_text.len()
}

/// Applies a `didChange` content change: the whole text, or a range.
fn apply_change(text: &mut String, change: &Value, utf8: bool) {
    let new = change["text"].as_str().unwrap_or_default();
    match change.get("range") {
        Some(range) => {
            let start = offset(text, &range["start"], utf8);
            let end = offset(text, &range["end"], utf8).max(start);
            text.replace_range(start..end, new);
        }
        None => *text = new.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(message: &Value) -> Vec<u8> {
        let body = message.to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    fn replies(output: &[u8]) -> Vec<Value> {
        let mut reader = BufReader::new(output);
        std::iter::from_fn(|| read_message(&mut reader).unwrap()).collect()
    }

    #[test]
    fn reports_each_marker_occurrence_in_utf16_or_utf8_columns() {
        let script = LspScript {
            markers: vec![Marker { text: "oops".into(), severity: 1, message: "bad".into(), code: Some(2322) }],
            ..LspScript::default()
        };
        for (encodings, column) in [(json!(["utf-16"]), 4), (json!(["utf-8", "utf-16"]), 7)] {
            let mut input = Vec::new();
            let init = json!({ "capabilities": { "general": { "positionEncodings": encodings } } });
            input.extend(frame(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": init })));
            let doc = json!({ "textDocument": { "uri": "file:///a.ts", "languageId": "typescript", "version": 1, "text": "é😀\noops" } });
            input.extend(frame(&json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": doc })));
            let change = json!({ "textDocument": { "uri": "file:///a.ts", "version": 2 }, "contentChanges": [{ "text": "é😀 oops" }] });
            input.extend(frame(&json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": change })));
            let pull = json!({ "textDocument": { "uri": "file:///a.ts" } });
            input.extend(frame(&json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/diagnostic", "params": pull })));
            let mut output = Vec::new();
            let mut seen = Vec::new();
            assert_eq!(serve(&Mutex::new(script.clone()), input.as_slice(), &mut output, |r| seen.push(r.method)), 0);

            let replies = replies(&output);
            let item = &replies[1]["result"]["items"][0];
            assert_eq!(item["range"]["start"], json!({ "line": 0, "character": column }));
            assert_eq!(item["code"], 2322);
            assert_eq!(seen, ["initialize", "textDocument/didOpen", "textDocument/didChange", "textDocument/diagnostic"]);
        }
    }

    #[test]
    fn crashes_on_its_method_without_answering() {
        let script = LspScript { crash_on: Some("initialize".into()), ..LspScript::default() };
        let input = frame(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }));
        let mut output = Vec::new();
        assert_eq!(serve(&Mutex::new(script.clone()), input.as_slice(), &mut output, |_| {}), 1);
        assert!(output.is_empty());
    }

    #[test]
    fn a_script_survives_json() {
        let script = LspScript {
            markers: vec![Marker { text: "x".into(), severity: 2, message: "m".into(), code: None }],
            crash_on: Some("initialized".into()),
            silent: true,
            delay: Duration::from_millis(5),
            flood: 3,
            watch: vec!["**/*.ts".into()],
        };
        assert_eq!(LspScript::from_json(&script.to_json()).unwrap(), script);
    }
}
