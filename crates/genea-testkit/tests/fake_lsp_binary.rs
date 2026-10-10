//! The fake LSP server binary reads its script from beside itself, as the
//! benchmark harness installs it in place of tsgo.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use genea_testkit::{FakeLsp, FixtureProject};
use serde_json::{Value, json};

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn read(output: &mut impl BufRead) -> Value {
    let mut length = 0;
    loop {
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        match line.trim_end().split_once(": ") {
            Some(("Content-Length", value)) => length = value.parse().unwrap(),
            _ if line.trim_end().is_empty() => break,
            _ => {}
        }
    }
    let mut body = vec![0; length];
    output.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn the_binary_plays_the_script_beside_it() {
    let folder = FixtureProject::new().build();
    let tsc = folder.path("lib/tsc");
    std::fs::create_dir_all(tsc.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_genea-fake-lsp"), &tsc).unwrap();
    let script = FakeLsp::new().error("oops", "Bad.").script();
    std::fs::write(folder.path("lib/tsc.json"), script.to_json()).unwrap();

    let mut child = Command::new(&tsc)
        .args(["--lsp", "--stdio"])
        .env_remove("GENEA_FAKE_LSP_SCRIPT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))).unwrap();
    assert_eq!(read(&mut output)["result"]["serverInfo"]["name"], "fake-lsp");
    let open = json!({ "textDocument": { "uri": "file:///a.ts", "languageId": "typescript", "version": 1, "text": "x oops" } });
    input.write_all(&frame(&json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": open }))).unwrap();
    let pull = json!({ "textDocument": { "uri": "file:///a.ts" } });
    input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/diagnostic", "params": pull }))).unwrap();
    let report = read(&mut output);
    assert_eq!(report["result"]["items"][0]["message"], "Bad.");
    assert_eq!(report["result"]["items"][0]["range"]["start"], json!({ "line": 0, "character": 2 }));

    input.write_all(&frame(&json!({ "jsonrpc": "2.0", "method": "exit" }))).unwrap();
    assert!(child.wait().unwrap().success());
}
