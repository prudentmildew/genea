//! Driving the real `genea` binary.
//!
//! With the journal on, Genea is started with `GENEA_JOURNAL=1` and its
//! stdin and stdout become the control channel
//! (`crates/genea-view/src/remote.rs`): one command per line in, one JSON
//! line back per command. Without the journal it is just started, and the
//! harness can only watch it from outside ([`sys::usage`]).

use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, RecvTimeoutError, channel},
    time::Duration,
};

use serde_json::Value;

use crate::{journal::Journal, keys::Key, sys};

/// How long a command may take before Genea is considered stuck.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Genea {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Option<Receiver<String>>,
}

/// What Genea reports about its window and display.
#[derive(Clone, Debug)]
pub struct Info {
    pub display_hz: f64,
    pub window_scale: f64,
    pub window_width: f64,
    pub window_height: f64,
    pub visible: bool,
}

impl Genea {
    /// Starts Genea on `file` in the project `workspace`, with or without
    /// the journal.
    pub fn launch(binary: &Path, workspace: &Path, file: &str, journal: bool) -> Result<Genea, String> {
        let mut command = Command::new(binary);
        command.arg(workspace).arg(file).stderr(Stdio::inherit());
        if journal {
            command.env("GENEA_JOURNAL", "1").stdin(Stdio::piped()).stdout(Stdio::piped());
        } else {
            command.env_remove("GENEA_JOURNAL").stdin(Stdio::null()).stdout(Stdio::null());
        }
        let mut child = command.spawn().map_err(|e| format!("couldn't start {}: {e}", binary.display()))?;
        let stdin = child.stdin.take();
        let replies = child.stdout.take().map(|stdout| {
            let (send, receive) = channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if send.send(line).is_err() {
                        break;
                    }
                }
            });
            receive
        });
        Ok(Genea { child, stdin, replies })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn usage(&self) -> Option<sys::Usage> {
        sys::usage(self.pid())
    }

    /// Sends one command and waits for its answer.
    pub fn send(&mut self, command: &str) -> Result<Value, String> {
        self.send_with_timeout(command, REPLY_TIMEOUT)
    }

    pub fn send_with_timeout(&mut self, command: &str, timeout: Duration) -> Result<Value, String> {
        let (Some(stdin), Some(replies)) = (self.stdin.as_mut(), self.replies.as_ref()) else {
            return Err("Genea was started without the journal".into());
        };
        writeln!(stdin, "{command}").and_then(|()| stdin.flush()).map_err(|e| format!("{command}: {e}"))?;
        let line = match replies.recv_timeout(timeout) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => return Err(format!("{command}: no answer within {timeout:?}")),
            Err(RecvTimeoutError::Disconnected) => return Err(format!("{command}: Genea exited")),
        };
        let value: Value = serde_json::from_str(&line).map_err(|e| format!("{command}: bad answer {line:?}: {e}"))?;
        match value.get("error").and_then(Value::as_str) {
            Some(error) => Err(format!("{command}: {error}")),
            None => Ok(value),
        }
    }

    /// Waits until the first frame with file content is presented in a
    /// visible window.
    pub fn wait_content(&mut self) -> Result<(), String> {
        self.send("wait-content").map(drop)
    }

    pub fn journal(&mut self) -> Result<Journal, String> {
        Journal::from_json(&self.send("journal")?)
    }

    pub fn info(&mut self) -> Result<Info, String> {
        let v = self.send("info")?;
        let number = |key: &str| v.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        Ok(Info {
            display_hz: number("display_hz"),
            window_scale: number("window_scale"),
            window_width: number("window_width"),
            window_height: number("window_height"),
            visible: v.get("visible").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// Presses and releases a key (posted into Genea's own event queue).
    pub fn key(&mut self, key: Key) -> Result<(), String> {
        self.send(&format!("key {} {}", key.code, key.flags)).map(drop)
    }

    /// Puts the caret at a 0-based line and display column.
    pub fn place_caret(&mut self, line: usize, column: usize) -> Result<(), String> {
        self.send(&format!("place-caret {line} {column}")).map(drop)
    }

    /// Scrolls `px` points per presented frame for `duration`, bouncing at
    /// the ends of the file.
    pub fn scroll(&mut self, px: f64, duration: Duration) -> Result<(), String> {
        self.send_with_timeout(&format!("scroll {px} {}", duration.as_millis()), duration + REPLY_TIMEOUT).map(drop)
    }

    /// Asks for a window size in points.
    pub fn resize(&mut self, width: f64, height: f64) -> Result<(), String> {
        self.send(&format!("resize {width} {height}")).map(drop)
    }

    /// Opens a file (relative to the project folder) in the first window;
    /// answers once a frame showing it has presented.
    pub fn open(&mut self, file: &str) -> Result<(), String> {
        self.send(&format!("open {file}")).map(drop)
    }

    /// The focused file: `path`, `line_count`, `loading` (the rest is still
    /// being read) and `large_file`.
    pub fn editor(&mut self) -> Result<Value, String> {
        self.send("editor")
    }

    /// The caret's line text and the caret's display column.
    pub fn caret_line(&mut self) -> Result<(String, usize, bool), String> {
        let v = self.send("caret-line")?;
        let text = v.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
        let column = v.get("column").and_then(Value::as_u64).unwrap_or(0) as usize;
        let composing = v.get("composing").and_then(Value::as_bool).unwrap_or(false);
        Ok((text, column, composing))
    }

    /// Opens the finder in a mode (`files`, `recent`, `actions`,
    /// `everywhere`), or closes it (`close`).
    pub fn finder(&mut self, mode: &str) -> Result<(), String> {
        self.send(&format!("finder {mode}")).map(drop)
    }

    /// Shows the Search view and searches the project for `text`; returns
    /// when Genea dispatched the search (ns since boot).
    pub fn search(&mut self, text: &str) -> Result<u64, String> {
        at(&self.send(&format!("search {text}"))?)
    }

    /// Expands or collapses a folder in the Files view; returns when Genea
    /// dispatched it (ns since boot).
    pub fn toggle_folder(&mut self, folder: &str) -> Result<u64, String> {
        at(&self.send(&format!("toggle-folder {folder}"))?)
    }

    /// Starts watching for a condition on the view state under `name`
    /// (`crates/genea-view/src/remote.rs` lists them). Only views synced
    /// from now on count.
    pub fn expect(&mut self, name: &str, condition: &str) -> Result<(), String> {
        self.send(&format!("expect {name} {condition}")).map(drop)
    }

    /// Waits until a frame drawn after the view met `name`'s condition is
    /// presented. After `timeout`, a condition met without a frame (nothing
    /// on screen changed) is [`Met::unchanged`]; one never met is an error.
    pub fn await_met(&mut self, name: &str, timeout: Duration) -> Result<Met, String> {
        let v = self.send_with_timeout(&format!("await {name} {}", timeout.as_millis()), timeout + REPLY_TIMEOUT)?;
        let at = v.get("met").and_then(Value::as_u64).ok_or_else(|| format!("await {name}: no time in {v}"))?;
        Ok(Met { at, unchanged: v.get("presented") == Some(&Value::Bool(false)) })
    }

    /// Whether the view meets a condition now.
    pub fn check(&mut self, condition: &str) -> Result<bool, String> {
        Ok(self.send(&format!("check {condition}"))?.get("holds").and_then(Value::as_bool).unwrap_or(false))
    }

    /// Genea's child processes and theirs: language servers, shells.
    pub fn processes(&mut self) -> Result<Vec<u32>, String> {
        let v = self.send("processes")?;
        Ok(v["pids"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|p| p as u32).collect())
    }

    /// Asks Genea to quit, or kills it without the journal, and waits.
    pub fn quit(mut self) {
        if self.stdin.is_some() {
            let _ = self.send_with_timeout("quit", Duration::from_secs(5));
        }
        self.stop();
    }

    fn stop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// When the view met a condition the harness waited for.
#[derive(Clone, Copy, Debug)]
pub struct Met {
    /// The end of the sync that met it, in ns since boot.
    pub at: u64,
    /// No frame came after it: nothing on screen changed (the finder's
    /// results for a longer query were the same, say).
    pub unchanged: bool,
}

/// The `at` time in a command's answer.
fn at(answer: &Value) -> Result<u64, String> {
    answer.get("at").and_then(Value::as_u64).ok_or_else(|| format!("no time in {answer}"))
}

impl Drop for Genea {
    fn drop(&mut self) {
        self.stop();
    }
}
