//! `genea-fake-lsp`: the fake LSP server as a binary, for the benchmark
//! harness, which drives the real Genea (see `genea_testkit::fake_lsp`).
//!
//! It reads its script (`LspScript` as JSON) from the file named in
//! `GENEA_FAKE_LSP_SCRIPT`, else from the file beside the binary named like
//! it plus `.json` (`lib/tsc` → `lib/tsc.json`), else it is a well-behaved
//! server that reports nothing. Genea gives language servers the login
//! shell's environment, not its own, so the file beside the binary is what
//! the harness uses.

use std::{fs, path::PathBuf, process::ExitCode};

use genea_testkit::{LspScript, fake_lsp};

fn main() -> ExitCode {
    let path = std::env::var_os("GENEA_FAKE_LSP_SCRIPT").map(PathBuf::from).or_else(|| {
        let exe = std::env::current_exe().ok()?;
        let mut name = exe.file_name()?.to_owned();
        name.push(".json");
        Some(exe.with_file_name(name))
    });
    let script = match path.map(fs::read_to_string) {
        Some(Ok(text)) => match LspScript::from_json(&text) {
            Ok(script) => script,
            Err(error) => {
                eprintln!("genea-fake-lsp: bad script: {error}");
                return ExitCode::from(2);
            }
        },
        _ => LspScript::default(),
    };
    let code = fake_lsp::serve(&std::sync::Mutex::new(script), std::io::stdin().lock(), std::io::stdout().lock(), |_| {});
    ExitCode::from(code.clamp(0, 255) as u8)
}
