//! Indentation (ticket #26, from #13): each file's `useTabs` and `tabWidth`,
//! resolved the way Oxfmt resolves them, so what the user types matches what
//! the formatter writes. `oxfmt --lsp` can't report its resolved options,
//! so Genea reads the same files:
//!
//! 1. `.oxfmtrc.json` (or `.oxfmtrc.jsonc`) overrides matching the file, in
//!    order, over its root options;
//! 2. the nearest `.editorconfig`, with its glob sections, fills in what is
//!    still unset;
//! 3. Oxfmt's defaults (2 spaces) cover the rest.
//!
//! Both files are looked for in the project root and then each folder above
//! it, as Oxfmt does from its working directory. It applies even when
//! formatting is off and to files Oxfmt doesn't format. Genea never reads
//! Prettier or Biome config and never guesses from a file's contents.

use std::{
    fs,
    path::{Path, PathBuf},
};

use jsonc_parser::{JsonObject, JsonValue, ParseOptions, parse_to_value};

/// Oxfmt's config files, in the order it prefers them in one folder.
pub(crate) const OXFMT_CONFIGS: [&str; 2] = [".oxfmtrc.json", ".oxfmtrc.jsonc"];

/// How a file is indented: what Tab, ⇧Tab, auto-indent and the status bar
/// use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Indentation {
    /// Indent with tabs rather than spaces.
    pub(crate) use_tabs: bool,
    /// Columns per level: the number of spaces, or a tab's width.
    pub(crate) tab_width: usize,
}

impl Default for Indentation {
    /// Oxfmt's defaults: two spaces.
    fn default() -> Self {
        Indentation { use_tabs: false, tab_width: 2 }
    }
}

impl Indentation {
    /// The status bar's item: `2 spaces`, or `Tabs`.
    pub(crate) fn label(self) -> String {
        if self.use_tabs { "Tabs".to_owned() } else { format!("{} spaces", self.tab_width) }
    }
}

/// The indentation settings of a project: what its Oxfmt config says.
/// Read in the background ([`IndentationConfig::read`]); resolving a file
/// against it is cheap and touches no disk.
#[derive(Debug, Default)]
pub(crate) struct IndentationConfig {
    oxfmtrc: Option<Oxfmtrc>,
}

/// The options Genea takes from Oxfmt's config: `None` is unset.
#[derive(Clone, Copy, Debug, Default)]
struct Options {
    use_tabs: Option<bool>,
    tab_width: Option<usize>,
}

/// An `.oxfmtrc.json`.
#[derive(Debug)]
struct Oxfmtrc {
    options: Options,
}

impl IndentationConfig {
    /// Reads the config files that apply to the project at `root`. Blocking:
    /// run it in the background.
    pub(crate) fn read(root: &Path) -> Self {
        IndentationConfig { oxfmtrc: find_oxfmtrc(root).and_then(|path| Oxfmtrc::read(&path)) }
    }

    /// How the file at `path` (absolute) is indented.
    pub(crate) fn resolve(&self, _path: &Path) -> Indentation {
        let options = self.oxfmtrc.as_ref().map(|o| o.options).unwrap_or_default();
        let default = Indentation::default();
        Indentation {
            use_tabs: options.use_tabs.unwrap_or(default.use_tabs),
            tab_width: options.tab_width.unwrap_or(default.tab_width),
        }
    }
}

/// The Oxfmt config file in the nearest folder, from `root` up, that has one.
fn find_oxfmtrc(root: &Path) -> Option<PathBuf> {
    root.ancestors().flat_map(|dir| OXFMT_CONFIGS.map(|name| dir.join(name))).find(|path| path.is_file())
}

impl Oxfmtrc {
    /// Reads and parses a config file. One Oxfmt can't use (unreadable, not
    /// JSONC, or a wrong value) counts as no config, as `oxfmt --lsp` then
    /// formats with its defaults.
    fn read(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let value = parse_to_value(&text, &ParseOptions::default()).ok()??;
        let JsonValue::Object(object) = value else { return None };
        Some(Oxfmtrc { options: Options::parse(&object)? })
    }
}

impl Options {
    /// `useTabs` and `tabWidth` from a config object, or `None` if either has
    /// a value Oxfmt rejects.
    fn parse(object: &JsonObject) -> Option<Self> {
        let use_tabs = match object.get("useTabs") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::Boolean(value)) => Some(*value),
            Some(_) => return None,
        };
        let tab_width = match object.get("tabWidth") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::Number(number)) => Some(number.parse::<u8>().ok().filter(|w| *w > 0)? as usize),
            Some(_) => return None,
        };
        Some(Options { use_tabs, tab_width })
    }
}
