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

use editorconfig_parser::{EditorConfig, EditorConfigProperty, IndentStyle};
use jsonc_parser::{JsonObject, JsonValue, ParseOptions, parse_to_value};

/// Oxfmt's config files, in the order it prefers them in one folder.
const OXFMT_CONFIGS: [&str; 2] = [".oxfmtrc.json", ".oxfmtrc.jsonc"];

/// The EditorConfig file. Like Oxfmt, Genea reads only the nearest one,
/// whether or not it says `root = true`.
const EDITORCONFIG: &str = ".editorconfig";

/// Whether a file with this name can change a project's indentation.
pub(crate) fn is_config_file(name: &str) -> bool {
    name == EDITORCONFIG || OXFMT_CONFIGS.contains(&name)
}

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
    /// One level of indentation, as typed: a tab, or `tab_width` spaces.
    pub(crate) fn unit(self) -> String {
        if self.use_tabs { "\t".to_owned() } else { " ".repeat(self.tab_width) }
    }

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
    /// The nearest `.editorconfig`, its section globs relative to its
    /// folder.
    editorconfig: Option<EditorConfig>,
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
    /// Its folder: override globs match paths relative to it.
    dir: PathBuf,
    options: Options,
    overrides: Vec<Override>,
}

/// An entry of `overrides`: options for the files its globs match.
#[derive(Debug)]
struct Override {
    files: Globs,
    exclude_files: Globs,
    options: Options,
}

/// Override globs, as Oxfmt normalises them: a pattern without a `/`
/// matches in any folder (`**/` goes in front), and a leading `./` anchors
/// it to the config's folder.
#[derive(Debug)]
struct Globs(Vec<String>);

impl IndentationConfig {
    /// Reads the config files that apply to the project at `root`. Blocking:
    /// run it in the background.
    pub(crate) fn read(root: &Path) -> Self {
        IndentationConfig {
            oxfmtrc: find_oxfmtrc(root).and_then(|path| Oxfmtrc::read(&path)),
            editorconfig: read_editorconfig(root),
        }
    }

    /// How the file at `path` (absolute) is indented.
    pub(crate) fn resolve(&self, path: &Path) -> Indentation {
        let mut options = self.oxfmtrc.as_ref().map(|o| o.options_for(path)).unwrap_or_default();
        if let Some(editorconfig) = &self.editorconfig {
            options.fill_from(editorconfig, path);
        }
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

/// The nearest `.editorconfig`, from `root` up. An unreadable one counts as
/// none.
fn read_editorconfig(root: &Path) -> Option<EditorConfig> {
    let path = root.ancestors().map(|dir| dir.join(EDITORCONFIG)).find(|path| path.is_file())?;
    let text = fs::read_to_string(&path).ok()?;
    Some(EditorConfig::parse(&text).with_cwd(path.parent()?))
}

impl Oxfmtrc {
    /// Reads and parses a config file. One Oxfmt can't use (unreadable, not
    /// JSONC, or a wrong value) counts as no config, as `oxfmt --lsp` then
    /// formats with its defaults.
    fn read(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let value = parse_to_value(&text, &ParseOptions::default()).ok()??;
        let JsonValue::Object(object) = value else { return None };
        let overrides = match object.get("overrides") {
            None | Some(JsonValue::Null) => Vec::new(),
            Some(JsonValue::Array(entries)) => entries.iter().map(Override::parse).collect::<Option<_>>()?,
            Some(_) => return None,
        };
        let dir = path.parent()?.to_path_buf();
        Some(Oxfmtrc { dir, options: Options::parse(&object)?, overrides })
    }

    /// The root options, with every override that matches `path` (absolute)
    /// merged over them in order.
    fn options_for(&self, path: &Path) -> Options {
        let relative = path.strip_prefix(&self.dir).unwrap_or(path).to_string_lossy();
        self.overrides
            .iter()
            .filter(|o| o.files.matches(&relative) && !o.exclude_files.matches(&relative))
            .fold(self.options, |options, o| Options {
                use_tabs: o.options.use_tabs.or(options.use_tabs),
                tab_width: o.options.tab_width.or(options.tab_width),
            })
    }
}

impl Override {
    fn parse(value: &JsonValue) -> Option<Self> {
        let JsonValue::Object(object) = value else { return None };
        let options = match object.get("options") {
            None | Some(JsonValue::Null) => Options::default(),
            Some(JsonValue::Object(options)) => Options::parse(options)?,
            Some(_) => return None,
        };
        Some(Override {
            files: Globs::parse(object.get("files"))?,
            exclude_files: Globs::parse(object.get("excludeFiles"))?,
            options,
        })
    }
}

impl Globs {
    /// A list of glob strings; absent is none. `None` if it isn't one or a
    /// pattern is invalid, which Oxfmt rejects.
    fn parse(value: Option<&JsonValue>) -> Option<Self> {
        let patterns = match value {
            None | Some(JsonValue::Null) => return Some(Globs(Vec::new())),
            Some(JsonValue::Array(patterns)) => patterns,
            Some(_) => return None,
        };
        let mut globs = Vec::new();
        for pattern in patterns.iter() {
            let JsonValue::String(pattern) = pattern else { return None };
            fast_glob::validate(pattern.as_bytes()).ok()?;
            globs.push(match pattern.strip_prefix("./") {
                Some(anchored) => anchored.to_owned(),
                None if pattern.contains('/') => pattern.to_string(),
                None => format!("**/{pattern}"),
            });
        }
        Some(Globs(globs))
    }

    fn matches(&self, relative: &str) -> bool {
        self.0.iter().any(|glob| fast_glob::glob_match(glob, relative))
    }
}

impl Options {
    /// Fills what is still unset from the `.editorconfig` sections matching
    /// `path` (absolute), as Oxfmt does: `indent_style` gives `useTabs`;
    /// `indent_size` gives `tabWidth` only when indenting with spaces
    /// (Prettier's rule), else `tab_width` does.
    fn fill_from(&mut self, editorconfig: &EditorConfig, path: &Path) {
        let properties = editorconfig.resolve(path);
        if self.use_tabs.is_none()
            && let EditorConfigProperty::Value(style) = properties.indent_style
        {
            self.use_tabs = Some(style == IndentStyle::Tab);
        }
        if self.tab_width.is_none() {
            let size = match (self.use_tabs, properties.indent_size) {
                (Some(false), EditorConfigProperty::Value(size)) => Some(size),
                _ => match properties.tab_width {
                    EditorConfigProperty::Value(width) => Some(width),
                    _ => None,
                },
            };
            self.tab_width = size.filter(|size| (1..=u8::MAX as usize).contains(size));
        }
    }

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
