//! Actions: the commands a user can run by name from Find Action (⌘⇧A)
//! and Search Everywhere (⇧⇧), each with its shortcut (ticket #33).
//!
//! An action is a [`Command`] that needs nothing but the project's state:
//! the menu items and the editor's shortcuts, not typing or caret moves.
//! The shortcuts are the labels of the WebStorm macOS keymap that
//! `genea-view` binds (its menus and `src/keys.rs`); keep them in step.
//! Menu items that only the app can do (Open…, About Genea) aren't actions
//! here.

use crate::{
    command::Command,
    view::{FinderMode, LeftColumnView, ToolchainPickerKind},
};

/// Something the user can run from the finder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Save,
    CloseTab,
    OpenConfig,
    ReloadEnvironment,
    InstallDependencies,
    RestartLanguageServer,
    SetRuntime,
    SetPackageManager,
    UpdateToolchain,
    RemoveUnusedToolchains,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    SelectNextOccurrence,
    UnselectLastOccurrence,
    SelectAllOccurrences,
    CloneCaretAbove,
    CloneCaretBelow,
    ToggleLineComment,
    ExpandSelection,
    ShrinkSelection,
    ExpandFold,
    CollapseFold,
    ExpandAllFolds,
    CollapseAllFolds,
    GoToFile,
    RecentFiles,
    FindAction,
    SearchEverywhere,
    ShowFiles,
    ShowSearch,
    ShowChanges,
    ShowProblems,
    ShowTerminal,
    SplitRight,
    MoveTabToOtherSide,
    CloseSplit,
    SelectNextTab,
    SelectPreviousTab,
}

impl Action {
    /// Every action, in the order Find Action lists them with an empty
    /// query: by menu.
    pub const ALL: [Action; 42] = [
        Action::Save,
        Action::CloseTab,
        Action::OpenConfig,
        Action::ReloadEnvironment,
        Action::InstallDependencies,
        Action::RestartLanguageServer,
        Action::SetRuntime,
        Action::SetPackageManager,
        Action::UpdateToolchain,
        Action::RemoveUnusedToolchains,
        Action::Undo,
        Action::Redo,
        Action::Cut,
        Action::Copy,
        Action::Paste,
        Action::SelectAll,
        Action::SelectNextOccurrence,
        Action::UnselectLastOccurrence,
        Action::SelectAllOccurrences,
        Action::CloneCaretAbove,
        Action::CloneCaretBelow,
        Action::ToggleLineComment,
        Action::ExpandSelection,
        Action::ShrinkSelection,
        Action::ExpandFold,
        Action::CollapseFold,
        Action::ExpandAllFolds,
        Action::CollapseAllFolds,
        Action::GoToFile,
        Action::RecentFiles,
        Action::FindAction,
        Action::SearchEverywhere,
        Action::ShowFiles,
        Action::ShowSearch,
        Action::ShowChanges,
        Action::ShowProblems,
        Action::ShowTerminal,
        Action::SplitRight,
        Action::MoveTabToOtherSide,
        Action::CloseSplit,
        Action::SelectNextTab,
        Action::SelectPreviousTab,
    ];

    /// The action's name, as menus show it.
    pub fn name(self) -> &'static str {
        match self {
            Action::Save => "Save",
            Action::CloseTab => "Close Tab",
            Action::OpenConfig => "Open Config",
            Action::ReloadEnvironment => "Reload Environment",
            Action::InstallDependencies => "Install Dependencies",
            Action::RestartLanguageServer => "Restart Language Server",
            Action::SetRuntime => "Set Runtime…",
            Action::SetPackageManager => "Set Package Manager…",
            Action::UpdateToolchain => "Update Toolchain…",
            Action::RemoveUnusedToolchains => "Remove Unused Toolchains",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Cut => "Cut",
            Action::Copy => "Copy",
            Action::Paste => "Paste",
            Action::SelectAll => "Select All",
            Action::SelectNextOccurrence => "Add Selection for Next Occurrence",
            Action::UnselectLastOccurrence => "Unselect Occurrence",
            Action::SelectAllOccurrences => "Select All Occurrences",
            Action::CloneCaretAbove => "Clone Caret Above",
            Action::CloneCaretBelow => "Clone Caret Below",
            Action::ToggleLineComment => "Comment with Line Comment",
            Action::ExpandSelection => "Expand Selection",
            Action::ShrinkSelection => "Shrink Selection",
            Action::ExpandFold => "Expand Fold",
            Action::CollapseFold => "Collapse Fold",
            Action::ExpandAllFolds => "Expand All Folds",
            Action::CollapseAllFolds => "Collapse All Folds",
            Action::GoToFile => "Go to File…",
            Action::RecentFiles => "Recent Files",
            Action::FindAction => "Find Action…",
            Action::SearchEverywhere => "Search Everywhere",
            Action::ShowFiles => "Files",
            Action::ShowSearch => "Search",
            Action::ShowChanges => "Changes",
            Action::ShowProblems => "Problems",
            Action::ShowTerminal => "Terminal",
            Action::SplitRight => "Split Right",
            Action::MoveTabToOtherSide => "Move Tab to Other Side",
            Action::CloseSplit => "Close Split",
            Action::SelectNextTab => "Select Next Tab",
            Action::SelectPreviousTab => "Select Previous Tab",
        }
    }

    /// The keyboard shortcut, as macOS menus write it (modifiers in the
    /// order ⌃⌥⇧⌘), if the action has one.
    pub fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Action::Save => "⌘S",
            Action::CloseTab => "⌘W",
            Action::Undo => "⌘Z",
            Action::Redo => "⇧⌘Z",
            Action::Cut => "⌘X",
            Action::Copy => "⌘C",
            Action::Paste => "⌘V",
            Action::SelectAll => "⌘A",
            Action::SelectNextOccurrence => "⌃G",
            Action::UnselectLastOccurrence => "⌃⇧G",
            Action::SelectAllOccurrences => "⌃⌘G",
            // Press ⌥ twice and hold it, then the arrow.
            Action::CloneCaretAbove => "⌥⌥↑",
            Action::CloneCaretBelow => "⌥⌥↓",
            Action::ToggleLineComment => "⌘/",
            Action::ExpandSelection => "⌥↑",
            Action::ShrinkSelection => "⌥↓",
            Action::ExpandFold => "⌥⌘=",
            Action::CollapseFold => "⌥⌘-",
            Action::GoToFile => "⇧⌘O",
            Action::RecentFiles => "⌘E",
            Action::FindAction => "⇧⌘A",
            Action::SearchEverywhere => "⇧⇧",
            Action::ShowFiles => "⌘1",
            Action::ShowSearch => "⇧⌘F",
            Action::ShowProblems => "⌘6",
            Action::ShowTerminal => "⌥F12",
            Action::SelectNextTab => "⇧⌘]",
            Action::SelectPreviousTab => "⇧⌘[",
            Action::OpenConfig
            | Action::ReloadEnvironment
            | Action::InstallDependencies
            | Action::RestartLanguageServer
            | Action::SetRuntime
            | Action::SetPackageManager
            | Action::UpdateToolchain
            | Action::RemoveUnusedToolchains
            | Action::ExpandAllFolds
            | Action::CollapseAllFolds
            | Action::SplitRight
            | Action::MoveTabToOtherSide
            | Action::CloseSplit
            // ⌘0 is zoom (spec #19), so Changes has no shortcut.
            | Action::ShowChanges => return None,
        })
    }

    /// Whether the action acts on the editor's text, carets or folds, so it
    /// doesn't apply while the terminal has the focus.
    pub(crate) fn edits(self) -> bool {
        matches!(
            self,
            Action::Undo
                | Action::Redo
                | Action::Cut
                | Action::Copy
                | Action::Paste
                | Action::SelectAll
                | Action::SelectNextOccurrence
                | Action::UnselectLastOccurrence
                | Action::SelectAllOccurrences
                | Action::CloneCaretAbove
                | Action::CloneCaretBelow
                | Action::ToggleLineComment
                | Action::ExpandSelection
                | Action::ShrinkSelection
                | Action::ExpandFold
                | Action::CollapseFold
                | Action::ExpandAllFolds
                | Action::CollapseAllFolds
        )
    }

    /// The command the action runs, for actions that don't depend on
    /// which tab is active. `None` for the tab actions.
    pub(crate) fn command(self) -> Option<Command> {
        Some(match self {
            Action::Save => Command::Save,
            Action::OpenConfig => Command::OpenConfig,
            Action::ReloadEnvironment => Command::ReloadEnvironment,
            Action::InstallDependencies => Command::InstallDependencies,
            Action::RestartLanguageServer => Command::RestartLanguageServer,
            Action::SetRuntime => Command::OpenToolchainPicker(ToolchainPickerKind::Runtime),
            Action::SetPackageManager => Command::OpenToolchainPicker(ToolchainPickerKind::PackageManager),
            Action::UpdateToolchain => Command::OpenToolchainPicker(ToolchainPickerKind::Update),
            Action::RemoveUnusedToolchains => Command::RemoveUnusedToolchains,
            Action::Undo => Command::Undo,
            Action::Redo => Command::Redo,
            Action::Cut => Command::Cut,
            Action::Copy => Command::Copy,
            Action::Paste => Command::Paste,
            Action::SelectAll => Command::SelectAll,
            Action::SelectNextOccurrence => Command::SelectNextOccurrence,
            Action::UnselectLastOccurrence => Command::UnselectLastOccurrence,
            Action::SelectAllOccurrences => Command::SelectAllOccurrences,
            Action::CloneCaretAbove => Command::CloneCaretAbove,
            Action::CloneCaretBelow => Command::CloneCaretBelow,
            Action::ToggleLineComment => Command::ToggleLineComment,
            Action::ExpandSelection => Command::ExpandSelection,
            Action::ShrinkSelection => Command::ShrinkSelection,
            Action::ExpandFold => Command::ExpandFold,
            Action::CollapseFold => Command::CollapseFold,
            Action::ExpandAllFolds => Command::ExpandAllFolds,
            Action::CollapseAllFolds => Command::CollapseAllFolds,
            Action::GoToFile => Command::OpenFinder(FinderMode::Files),
            Action::RecentFiles => Command::OpenFinder(FinderMode::RecentFiles),
            Action::FindAction => Command::OpenFinder(FinderMode::Actions),
            Action::SearchEverywhere => Command::OpenFinder(FinderMode::Everywhere),
            Action::ShowFiles => Command::ToggleLeftColumn(LeftColumnView::Files),
            Action::ShowSearch => Command::ToggleLeftColumn(LeftColumnView::Search),
            Action::ShowChanges => Command::ToggleLeftColumn(LeftColumnView::Changes),
            Action::ShowProblems => Command::ToggleLeftColumn(LeftColumnView::Problems),
            Action::ShowTerminal => Command::ToggleTerminal,
            Action::SplitRight => Command::SplitRight,
            Action::CloseSplit => Command::CloseSplit,
            Action::CloseTab | Action::MoveTabToOtherSide | Action::SelectNextTab | Action::SelectPreviousTab => {
                return None;
            }
        })
    }
}
