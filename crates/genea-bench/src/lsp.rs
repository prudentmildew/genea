//! The harness's own language-server client (ticket #63).
//!
//! Genea has no completions or go-to-definition yet (#43, #44), so the
//! end-to-end targets for them are timed against the project's tsgo
//! directly: the same binary, arguments and initialization Genea uses
//! (`crates/genea-core/src/project/language.rs`), request written to
//! response read. That is the language server's share of the target, which
//! is most of it; once Genea has the features, time them through Genea.
//!
//! JSON-RPC over stdio with LSP's `Content-Length` framing. Requests the
//! server sends (`workspace/configuration`, `client/registerCapability`, …)
//! are answered with nulls, as Genea answers them; notifications are dropped.

use std::{
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, RecvTimeoutError, channel},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// One message with its `Content-Length` header.
pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    framed.extend(body.as_bytes());
    framed
}

/// The next message, or `None` once the server closed its output.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
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
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// A running language server.
pub struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: u64,
}

/// How long a request may take before the server is considered stuck.
const TIMEOUT: Duration = Duration::from_secs(60);

impl Client {
    /// Starts `binary --lsp --stdio` in `root` and initializes it for
    /// `root`, as Genea does.
    pub fn start(binary: &Path, root: &Path) -> Result<Client, String> {
        let mut child = Command::new(binary)
            .args(["--lsp", "--stdio"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start {}: {e}", binary.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (send, messages) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut reader) {
                if send.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Client { child, stdin, messages, next_id: 0 };
        let uri = file_uri(root);
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        client.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": { "name": "genea-bench" },
                "rootUri": uri,
                "workspaceFolders": [{ "uri": uri, "name": name }],
                "capabilities": {
                    "general": { "positionEncodings": ["utf-8", "utf-16"] },
                    "workspace": { "configuration": true, "workspaceFolders": true },
                    "textDocument": {
                        "completion": { "completionItem": { "snippetSupport": false } },
                        "definition": { "linkSupport": true },
                    },
                },
                "initializationOptions": { "disablePushDiagnostics": true },
            }),
        )?;
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Opens a TypeScript file with `text`.
    pub fn open(&mut self, path: &Path, text: &str) -> Result<(), String> {
        let document = json!({ "uri": file_uri(path), "languageId": "typescript", "version": 1, "text": text });
        self.notify("textDocument/didOpen", json!({ "textDocument": document }))
    }

    /// Sends a request and waits for its answer: the result and how long it
    /// took, from writing the request to reading the response.
    pub fn request(&mut self, method: &str, params: Value) -> Result<(Value, Duration), String> {
        self.next_id += 1;
        let id = self.next_id;
        let started = Instant::now();
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let left = TIMEOUT.saturating_sub(started.elapsed());
            let message = match self.messages.recv_timeout(left) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => return Err(format!("{method}: no answer within {TIMEOUT:?}")),
                Err(RecvTimeoutError::Disconnected) => return Err(format!("{method}: the server exited")),
            };
            let took = started.elapsed();
            match (message.get("id"), message.get("method")) {
                // A request from the server: answer it as Genea does.
                (Some(server_id), Some(server_method)) => {
                    let result = match server_method.as_str() {
                        Some("workspace/configuration") => {
                            let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                            Value::Array(vec![Value::Null; items])
                        }
                        _ => Value::Null,
                    };
                    self.send(&json!({ "jsonrpc": "2.0", "id": server_id, "result": result }))?;
                }
                (Some(answer), None) if answer == id => {
                    return match message.get("error") {
                        Some(error) => Err(format!("{method}: {error}")),
                        None => Ok((message["result"].clone(), took)),
                    };
                }
                _ => {}
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        self.stdin.write_all(&encode(message)).and_then(|()| self.stdin.flush()).map_err(|e| format!("lsp: {e}"))
    }

    /// Asks the server to shut down, and kills it if it doesn't.
    pub fn stop(mut self) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// A `file:` URI for an absolute path (the paths here need no escaping
/// beyond spaces).
pub fn file_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy().replace(' ', "%20"))
}
