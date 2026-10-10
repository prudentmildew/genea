//! What a language server draws on top of the text (ticket #46): semantic
//! tokens, which refine the tree-sitter highlighting, and the inlay hints
//! and code lenses that the `inlayHints` and `codeLens` config keys turn
//! on.
//!
//! Each open document wants each kind again whenever its text changes (or
//! the server asks for a refresh), with one request of a kind in flight per
//! document. An answer carries the editor version it was asked for, and the
//! project applies it only if the editor is still at that version: one
//! for older text would land in the wrong places. Meanwhile the editor keeps
//! the last ones, moved with the edits, so typing never waits on the
//! server.

use std::{collections::BTreeMap, path::PathBuf};

use ropey::Rope;
use serde_json::{Value, json};

use super::{CONTENT_MODIFIED, Document, SERVER_CANCELLED, connection::{Connection, ResponseError}};
use super::text::Encoding;
use crate::syntax::Highlight;

/// The semantic token types Genea offers (LSP's standard ones). The server
/// answers with its own legend, a subset in its own order.
const TOKEN_TYPES: [&str; 23] = [
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];
const TOKEN_MODIFIERS: [&str; 10] = [
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

/// Adds what Genea understands of semantic tokens, inlay hints and code
/// lenses to the `initialize` params' capabilities.
pub(super) fn add_capabilities(params: &mut Value) {
    let capabilities = &mut params["capabilities"];
    let text_document = &mut capabilities["textDocument"];
    text_document["semanticTokens"] = json!({
        "dynamicRegistration": false,
        "requests": { "full": true },
        "tokenTypes": TOKEN_TYPES,
        "tokenModifiers": TOKEN_MODIFIERS,
        "formats": ["relative"],
        "overlappingTokenSupport": false,
        "multilineTokenSupport": false,
    });
    text_document["inlayHint"] = json!({ "dynamicRegistration": false });
    text_document["codeLens"] = json!({ "dynamicRegistration": false });
    let workspace = &mut capabilities["workspace"];
    workspace["semanticTokens"] = json!({ "refreshSupport": true });
    workspace["inlayHint"] = json!({ "refreshSupport": true });
    workspace["codeLens"] = json!({ "refreshSupport": true });
}

/// The answer to a server's `workspace/configuration` request (on the
/// reader thread): the settings tsgo reads for inlay hints and code lenses
/// (sections `typescript`, `javascript` and `js/ts`), all on. Whether they
/// show is up to the config keys, which decide whether Genea asks for them
/// at all, so a config change needs no `didChangeConfiguration`. Other
/// sections get `null`.
pub(super) fn configuration(params: &Value) -> Value {
    let settings = json!({
        "inlayHints": {
            "parameterNames": { "enabled": "all", "suppressWhenArgumentMatchesName": true },
            "parameterTypes": { "enabled": true },
            "variableTypes": { "enabled": true, "suppressWhenTypeMatchesName": true },
            "propertyDeclarationTypes": { "enabled": true },
            "functionLikeReturnTypes": { "enabled": true },
            "enumMemberValues": { "enabled": true },
        },
        "referencesCodeLens": { "enabled": true, "showOnAllFunctions": false },
        "implementationsCodeLens": { "enabled": true },
    });
    let items = params["items"].as_array().map(Vec::as_slice).unwrap_or_default();
    let answers = items.iter().map(|item| match item["section"].as_str() {
        Some("typescript" | "javascript" | "js/ts") => settings.clone(),
        _ => Value::Null,
    });
    Value::Array(answers.collect())
}

/// The kinds of decorations, each its own request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `textDocument/semanticTokens/full`.
    Tokens,
    /// `textDocument/inlayHint`, while the `inlayHints` config key is on.
    Hints,
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::Tokens, Kind::Hints];

    fn method(self) -> &'static str {
        match self {
            Kind::Tokens => "textDocument/semanticTokens/full",
            Kind::Hints => "textDocument/inlayHint",
        }
    }
}

/// A request for a document's decorations, waiting for its answer: for
/// the editor's `version`.
#[derive(Debug)]
pub(crate) struct Pending {
    kind: Kind,
    path: PathBuf,
    version: u64,
}

/// What the project applies to an open editor. Positions are in the
/// server's encoding, against the text at `version`.
#[derive(Debug)]
pub(crate) enum Output {
    Tokens { path: PathBuf, version: u64, encoding: Encoding, tokens: Vec<Token> },
    Hints { path: PathBuf, version: u64, encoding: Encoding, hints: Vec<Hint> },
    /// The server is gone: drop every editor's decorations.
    ClearAll,
}

/// A semantic token Genea colours: one line, in server units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) line: u32,
    pub(crate) start: u32,
    pub(crate) length: u32,
    pub(crate) highlight: Highlight,
}

/// An inlay hint: its text (padding included) and where it goes, in
/// server units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hint {
    pub(crate) line: u32,
    pub(crate) character: u32,
    pub(crate) label: String,
}

/// Hints longer than this are cut, as VS Code cuts them: a big object
/// type would otherwise push the line's own text far off.
const MAX_HINT_CHARS: usize = 43;

/// One kind of request for one document.
#[derive(Debug, Default)]
struct Want {
    /// Should be asked (again).
    wanted: bool,
    /// The request in flight.
    in_flight: Option<i64>,
}

#[derive(Debug, Default)]
struct Wants {
    tokens: Want,
    hints: Want,
    /// The document's line count, for the inlay hints' range.
    lines: u32,
}

impl Wants {
    fn of(&mut self, kind: Kind) -> &mut Want {
        match kind {
            Kind::Tokens => &mut self.tokens,
            Kind::Hints => &mut self.hints,
        }
    }
}

/// A server's decorations state, on the main thread.
#[derive(Default)]
pub(crate) struct Decorations {
    /// The server's semantic token legend; `None` if it has no semantic
    /// tokens.
    legend: Option<Legend>,
    /// The server has inlay hints.
    has_hints: bool,
    /// The `inlayHints` config key.
    show_hints: bool,
    documents: BTreeMap<PathBuf, Wants>,
}

/// What the server's token type and modifier numbers mean.
#[derive(Debug)]
struct Legend {
    types: Vec<String>,
    modifiers: Vec<String>,
}

/// Whether a capability is there (`true` or an options object).
fn provided(capability: &Value) -> bool {
    !matches!(capability, Value::Null | Value::Bool(false))
}

impl Decorations {
    /// The server answered `initialize`: what it provides.
    pub(super) fn initialized(&mut self, result: &Value) {
        let capabilities = &result["capabilities"];
        let provider = &capabilities["semanticTokensProvider"];
        let strings = |v: &Value| -> Vec<String> {
            v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
        };
        self.legend = provided(&provider["full"]).then(|| Legend {
            types: strings(&provider["legend"]["tokenTypes"]),
            modifiers: strings(&provider["legend"]["tokenModifiers"]),
        });
        self.has_hints = provided(&capabilities["inlayHintProvider"]);
    }

    /// What the config shows. Turning a kind on asks for it everywhere;
    /// turning it off drops answers still on their way (the project hides
    /// what the editors have).
    pub(super) fn show(&mut self, hints: bool) {
        if hints && !self.show_hints {
            self.want_everywhere(Kind::Hints);
        }
        self.show_hints = hints;
    }

    /// Whether this kind is asked for at all.
    fn active(&self, kind: Kind) -> bool {
        match kind {
            Kind::Tokens => self.legend.is_some(),
            Kind::Hints => self.has_hints && self.show_hints,
        }
    }

    /// A document was opened or its text changed (it has `lines` lines
    /// now): everything is wanted again.
    pub(super) fn changed(&mut self, path: &std::path::Path, lines: usize) {
        let wants = self.documents.entry(path.to_owned()).or_default();
        wants.lines = u32::try_from(lines).unwrap_or(u32::MAX);
        for kind in Kind::ALL {
            wants.of(kind).wanted = true;
        }
    }

    pub(super) fn closed(&mut self, path: &std::path::Path) {
        self.documents.remove(path);
    }

    fn want_everywhere(&mut self, kind: Kind) {
        for wants in self.documents.values_mut() {
            wants.of(kind).wanted = true;
        }
    }

    /// The server asked for a refresh (`workspace/semanticTokens/refresh`,
    /// …): every document wants that kind again. `false` for other methods.
    pub(super) fn refresh(&mut self, method: &str) -> bool {
        let kind = match method {
            "workspace/semanticTokens/refresh" => Kind::Tokens,
            "workspace/inlayHint/refresh" => Kind::Hints,
            _ => return false,
        };
        self.want_everywhere(kind);
        true
    }

    /// Sends what is wanted and not in flight.
    pub(super) fn request(
        &mut self,
        connection: &mut Connection,
        documents: &BTreeMap<PathBuf, Document>,
        requests: &mut std::collections::HashMap<i64, super::Pending>,
    ) {
        let active: Vec<Kind> = Kind::ALL.into_iter().filter(|kind| self.active(*kind)).collect();
        for (path, wants) in &mut self.documents {
            let Some(document) = documents.get(path) else { continue };
            for &kind in &active {
                let lines = wants.lines;
                let want = wants.of(kind);
                if !want.wanted || want.in_flight.is_some() {
                    continue;
                }
                want.wanted = false;
                let mut params = json!({ "textDocument": { "uri": document.uri } });
                if kind == Kind::Hints {
                    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": lines, "character": 0 } });
                    params["range"] = range;
                }
                let id = connection.request(kind.method(), params);
                want.in_flight = Some(id);
                let pending = Pending { kind, path: path.clone(), version: document.editor_version };
                requests.insert(id, super::Pending::Decorations(pending));
            }
        }
    }

    /// Handles an answer.
    pub(super) fn answered(
        &mut self,
        pending: Pending,
        id: i64,
        result: Result<Value, ResponseError>,
        encoding: Encoding,
    ) -> Vec<Output> {
        let Pending { kind, path, version } = pending;
        let active = self.active(kind);
        let Some(wants) = self.documents.get_mut(&path).filter(|w| w.of_ref(kind).in_flight == Some(id)) else {
            return Vec::new();
        };
        wants.of(kind).in_flight = None;
        let result = match result {
            Ok(result) if active => result,
            Ok(_) => return Vec::new(),
            Err(error) => {
                if retry(&error) {
                    wants.of(kind).wanted = true;
                }
                return Vec::new();
            }
        };
        match kind {
            Kind::Tokens => {
                let Some(legend) = &self.legend else { return Vec::new() };
                let data: Vec<u32> =
                    result["data"].as_array().into_iter().flatten().map(|n| n.as_u64().unwrap_or(0) as u32).collect();
                vec![Output::Tokens { path, version, encoding, tokens: legend.decode(&data) }]
            }
            Kind::Hints => {
                let hints = result.as_array().into_iter().flatten().filter_map(hint).collect();
                vec![Output::Hints { path, version, encoding, hints }]
            }
        }
    }
}

impl Wants {
    fn of_ref(&self, kind: Kind) -> &Want {
        match kind {
            Kind::Tokens => &self.tokens,
            Kind::Hints => &self.hints,
        }
    }
}

/// An `InlayHint` as Genea draws it: the label's parts joined, on one
/// line, cut at [`MAX_HINT_CHARS`], with its padding as spaces (except
/// before a type annotation's colon, which TypeScript writes right after
/// the name).
fn hint(value: &Value) -> Option<Hint> {
    let position = &value["position"];
    let line = u32::try_from(position["line"].as_u64()?).ok()?;
    let character = u32::try_from(position["character"].as_u64()?).ok()?;
    let label: String = match &value["label"] {
        Value::String(label) => label.clone(),
        Value::Array(parts) => parts.iter().filter_map(|part| part["value"].as_str()).collect(),
        _ => return None,
    };
    let mut label: String = label.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if label.chars().count() > MAX_HINT_CHARS {
        label = label.chars().take(MAX_HINT_CHARS - 1).chain(['…']).collect();
    }
    if value["paddingLeft"] == true && !label.starts_with(':') {
        label.insert(0, ' ');
    }
    if value["paddingRight"] == true {
        label.push(' ');
    }
    (!label.is_empty()).then_some(Hint { line, character, label })
}

/// The byte offset of a server position in `text`: the line, and the
/// column in the server's units. A line past the end, or a column past the
/// end of its line, is clamped.
pub(crate) fn byte_offset(text: &Rope, line: u32, character: u32, encoding: Encoding) -> usize {
    let line = (line as usize).min(text.len_lines().saturating_sub(1));
    let start = text.line_to_byte(line);
    if encoding == Encoding::Utf8 {
        // Bytes already: clamp to the line's text and to a char boundary.
        let slice = text.line(line);
        let mut len = slice.len_chars();
        while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
            len -= 1;
        }
        let ending = slice.len_chars() - len;
        let byte = start + (character as usize).min(slice.len_bytes() - ending);
        return text.char_to_byte(text.byte_to_char(byte));
    }
    let mut units = 0;
    let mut byte = start;
    for c in text.line(line).chars() {
        if c == '\n' || c == '\r' || units >= character as usize {
            break;
        }
        units += match encoding {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        };
        byte += c.len_utf8();
    }
    byte
}

/// Whether a failed request should be asked again: the server cancelled
/// it or the content changed, and it doesn't say not to.
fn retry(error: &ResponseError) -> bool {
    let retrigger = error.data["retriggerRequest"].as_bool().unwrap_or(true);
    matches!(error.code, SERVER_CANCELLED | CONTENT_MODIFIED) && retrigger
}

impl Legend {
    /// Decodes relative-encoded token data, keeping the tokens Genea
    /// colours differently from tree-sitter.
    fn decode(&self, data: &[u32]) -> Vec<Token> {
        let (mut line, mut start) = (0, 0);
        let mut tokens = Vec::new();
        for token in data.chunks_exact(5) {
            let [delta_line, delta_start, length, token_type, modifiers] = token else { continue };
            if *delta_line > 0 {
                line += delta_line;
                start = 0;
            }
            start += delta_start;
            let Some(token_type) = self.types.get(*token_type as usize) else { continue };
            let has = |name: &str| {
                self.modifiers.iter().position(|m| m == name).is_some_and(|bit| bit < 32 && modifiers & (1 << bit) != 0)
            };
            if let Some(highlight) = highlight(token_type, has("defaultLibrary"))
                && *length > 0
            {
                tokens.push(Token { line, start, length: *length, highlight });
            }
        }
        tokens
    }
}

/// How a semantic token type is coloured, or `None` to leave tree-sitter's
/// colour (variables, which tree-sitter already tells apart, and the
/// token types it knows better: keywords, strings, comments, …).
fn highlight(token_type: &str, default_library: bool) -> Option<Highlight> {
    Some(match token_type {
        "namespace" | "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" => Highlight::Type,
        "parameter" => Highlight::Parameter,
        "variable" if default_library => Highlight::VariableBuiltin,
        "property" => Highlight::Property,
        "enumMember" => Highlight::Constant,
        "function" | "method" => Highlight::Function,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_decode_against_the_servers_legend() {
        let legend = Legend {
            types: vec!["variable".into(), "parameter".into(), "interface".into(), "keyword".into()],
            modifiers: vec!["declaration".into(), "defaultLibrary".into()],
        };
        // `x` (parameter) at 0:2, `T` (interface) at 0:5, `console`
        // (default-library variable) at 2:4, `if` (keyword) at 2:12, `y`
        // (plain variable) at 3:0.
        let data = [0, 2, 1, 1, 1, 0, 3, 1, 2, 0, 2, 4, 7, 0, 2, 0, 8, 2, 3, 0, 1, 0, 1, 0, 0];
        assert_eq!(
            legend.decode(&data),
            [
                Token { line: 0, start: 2, length: 1, highlight: Highlight::Parameter },
                Token { line: 0, start: 5, length: 1, highlight: Highlight::Type },
                Token { line: 2, start: 4, length: 7, highlight: Highlight::VariableBuiltin },
            ]
        );
    }
}
