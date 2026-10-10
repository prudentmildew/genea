//! A project's side of what the language server draws on open files
//! (ticket #46): semantic highlights into the editors. The protocol side is
//! `crate::lsp::decorations`, the editors' side `editor/decorations.rs`.

use super::Project;
use crate::lsp::decorations::{Output, byte_offset};

impl Project {
    /// Applies a language server's decorations to the open editors.
    pub(super) fn apply_decorations(&mut self, output: Output) {
        match output {
            Output::Tokens { path, version, encoding, tokens } => {
                let Some(editor) = self.open_editor_mut(&path) else { return };
                let text = editor.text();
                let highlights = tokens
                    .iter()
                    .map(|token| {
                        let start = byte_offset(text, token.line, token.start, encoding);
                        let end = byte_offset(text, token.line, token.start + token.length, encoding);
                        (start..end, token.highlight)
                    })
                    .collect();
                editor.set_semantic_highlights(version, highlights);
            }
            Output::ClearAll => {
                let paths: Vec<_> = self.open_editors().map(|e| e.path().to_owned()).collect();
                for path in paths {
                    if let Some(editor) = self.open_editor_mut(&path) {
                        editor.clear_decorations();
                    }
                }
            }
        }
    }
}
