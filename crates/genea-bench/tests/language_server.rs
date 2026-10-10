//! The harness's own language-server client (ticket #63), which times
//! completions and go-to-definition against tsgo: JSON-RPC over stdio with
//! LSP's `Content-Length` framing.

use std::io::BufReader;

use genea_bench::lsp::{encode, read_message};
use serde_json::json;

#[test]
fn a_message_is_framed_with_its_length_in_bytes() {
    let framed = encode(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": { "ø": 1 } }));
    let text = String::from_utf8(framed).unwrap();
    let (header, body) = text.split_once("\r\n\r\n").unwrap();
    assert_eq!(header, format!("Content-Length: {}", body.len()));
    assert_eq!(body.len(), body.chars().count() + 1, "ø is two bytes");
}

#[test]
fn messages_are_read_back_one_by_one_whatever_headers_come_with_them() {
    let mut bytes = encode(&json!({ "id": 1, "result": null }));
    bytes.extend(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 14\r\n\r\n{\"id\":2,\"x\":3}");
    let mut reader = BufReader::new(&bytes[..]);

    assert_eq!(read_message(&mut reader).unwrap(), Some(json!({ "id": 1, "result": null })));
    assert_eq!(read_message(&mut reader).unwrap(), Some(json!({ "id": 2, "x": 3 })));
    assert_eq!(read_message(&mut reader).unwrap(), None, "the server closed its output");
}
