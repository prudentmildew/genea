//! A project's side of what the language server draws on open files
//! (ticket #46): semantic highlights and inlay hints into the editors, and
//! the `inlayHints` config key. The protocol side is
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
            Output::Hints { path, version, encoding, hints } => {
                let Some(editor) = self.open_editor_mut(&path) else { return };
                let text = editor.text();
                let hints = hints
                    .into_iter()
                    .map(|hint| (byte_offset(text, hint.line, hint.character, encoding), hint.label))
                    .collect();
                editor.set_inlay_hints(version, hints);
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

    /// Hides what the config turns off. Runs before every language sync,
    /// so a config change applies at once; cheap when nothing shows.
    pub(super) fn hide_decorations(&mut self) {
        if self.config.inlay_hints {
            return;
        }
        let shown: Vec<_> = self.open_editors().filter(|e| e.has_inlay_hints()).map(|e| e.path().to_owned()).collect();
        for path in shown {
            if let Some(editor) = self.open_editor_mut(&path) {
                editor.clear_inlay_hints();
            }
        }
    }
}
