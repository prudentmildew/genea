//! The languages Genea highlights, how a file's language is chosen, and
//! each language's grammar and queries.

use std::{path::Path, sync::OnceLock};

use tree_sitter::Query;

use super::highlight::{Paint, capture_paint};

/// A language with syntax highlighting: the first-class languages and the
/// basic file types (GLOSSARY.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Language {
    TypeScript,
    Tsx,
    /// JavaScript and JSX: one grammar covers both.
    JavaScript,
    Json,
    Css,
    Html,
    Markdown,
    /// Markdown's inline grammar (emphasis, code spans, links), embedded in
    /// each paragraph of the block grammar. Never a file's language.
    MarkdownInline,
    Yaml,
    /// `.env` files: no grammar, a line highlighter (`dotenv.rs`).
    Env,
}

impl Language {
    /// The language of a file, by its name. Other files are plain text.
    pub(crate) fn of_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?;
        if name == ".env" || name.starts_with(".env.") {
            return Some(Language::Env);
        }
        let extension = name.rsplit_once('.')?.1;
        Language::from_extension(&extension.to_ascii_lowercase())
    }

    fn from_extension(extension: &str) -> Option<Language> {
        Some(match extension {
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "json" | "jsonc" => Language::Json,
            "css" => Language::Css,
            "html" | "htm" => Language::Html,
            "md" | "markdown" => Language::Markdown,
            "yaml" | "yml" => Language::Yaml,
            _ => return None,
        })
    }

    /// The language an injection query or a Markdown code fence names.
    pub(crate) fn from_name(name: &str) -> Option<Language> {
        match name.to_ascii_lowercase().as_str() {
            "javascript" => Some(Language::JavaScript),
            "typescript" => Some(Language::TypeScript),
            "markdown_inline" => Some(Language::MarkdownInline),
            "env" | "dotenv" => None,
            other => Language::from_extension(other),
        }
    }

    /// The grammar and queries, built the first time a language is used.
    /// Compiling a query takes milliseconds, so only background parses call
    /// this. `None` for `.env`, which has no grammar.
    pub(crate) fn config(self) -> Option<&'static LanguageConfig> {
        static CONFIGS: [OnceLock<Option<LanguageConfig>>; 10] = [const { OnceLock::new() }; 10];
        CONFIGS[self as usize].get_or_init(|| LanguageConfig::new(self)).as_ref()
    }
}

/// A grammar and its compiled queries.
pub(crate) struct LanguageConfig {
    pub(crate) language: tree_sitter::Language,
    pub(crate) highlights: Query,
    /// What each of `highlights`' captures paints, by capture index.
    pub(super) paints: Vec<Option<Paint>>,
    pub(super) injections: Option<Query>,
}

impl LanguageConfig {
    fn new(language: Language) -> Option<Self> {
        let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
        let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
        let ts = tree_sitter_typescript::HIGHLIGHTS_QUERY;
        // The TypeScript queries extend the JavaScript ones, and later
        // patterns win, so they go last.
        let (grammar, highlights, injections) = match language {
            Language::TypeScript => {
                (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), format!("{js}\n{ts}"), None)
            }
            Language::Tsx => (tree_sitter_typescript::LANGUAGE_TSX.into(), format!("{js}\n{jsx}\n{ts}"), None),
            Language::JavaScript => (tree_sitter_javascript::LANGUAGE.into(), format!("{js}\n{jsx}"), None),
            Language::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into(), None),
            Language::Css => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into(), None),
            Language::Html => (
                tree_sitter_html::LANGUAGE.into(),
                tree_sitter_html::HIGHLIGHTS_QUERY.into(),
                Some(tree_sitter_html::INJECTIONS_QUERY),
            ),
            Language::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
                Some(tree_sitter_md::INJECTION_QUERY_BLOCK),
            ),
            Language::MarkdownInline => (
                tree_sitter_md::INLINE_LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into(),
                Some(tree_sitter_md::INJECTION_QUERY_INLINE),
            ),
            Language::Yaml => (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into(), None),
            Language::Env => return None,
        };
        let compile = |source: &str| {
            Query::new(&grammar, source)
                .map_err(|error| eprintln!("genea: the {language:?} query doesn't compile: {error}"))
                .ok()
        };
        let highlights = compile(&highlights)?;
        let paints = highlights.capture_names().iter().map(|name| capture_paint(name)).collect();
        let injections = injections.and_then(compile);
        Some(LanguageConfig { language: grammar, highlights, paints, injections })
    }
}
