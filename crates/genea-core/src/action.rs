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
    NewProject,
    Save,
    CloseTab,
    OpenConfig,
    ReloadEnvironment,
    InstallDependencies,
    RestartLanguageServer,
    RunProjectCheck,
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
    ShowQuickFixes,
    OrganizeImports,
    ReformatFile,
    ExpandSelection,
    ShrinkSelection,
    ExpandFold,
    CollapseFold,
    ExpandAllFolds,
    CollapseAllFolds,
    CodeCompletion,
    QuickDocumentation,
    ParameterInfo,
    GoToFile,
    RecentFiles,
    FindAction,
    SearchEverywhere,
    FileStructure,
    GoToSymbol,
    GoToDefinition,
    GoToTypeDefinition,
    GoToImplementation,
    FindUsages,
    Rename,
    ShowFiles,
    ShowSearch,
    ShowChanges,
    ShowProblems,
    ShowScripts,
    ShowTerminal,
    NewTerminalTab,
    SplitRight,
    MoveTabToOtherSide,
    CloseSplit,
    SelectNextTab,
    SelectPreviousTab,
}

impl Action {
    /// Every action, in the order Find Action lists them with an empty
    /// query: by menu.
    pub const ALL: [Action; 59] = [
        Action::NewProject,
        Action::Save,
        Action::CloseTab,
        Action::OpenConfig,
        Action::ReloadEnvironment,
        Action::InstallDependencies,
        Action::RestartLanguageServer,
        Action::RunProjectCheck,
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
        Action::ShowQuickFixes,
        Action::OrganizeImports,
        Action::ReformatFile,
        Action::ExpandSelection,
        Action::ShrinkSelection,
        Action::ExpandFold,
        Action::CollapseFold,
        Action::ExpandAllFolds,
        Action::CollapseAllFolds,
        Action::CodeCompletion,
        Action::QuickDocumentation,
        Action::ParameterInfo,
        Action::GoToFile,
        Action::RecentFiles,
        Action::FindAction,
        Action::SearchEverywhere,
        Action::FileStructure,
        Action::GoToSymbol,
        Action::GoToDefinition,
        Action::GoToTypeDefinition,
        Action::GoToImplementation,
        Action::FindUsages,
        Action::Rename,
        Action::ShowFiles,
        Action::ShowSearch,
        Action::ShowChanges,
        Action::ShowProblems,
        Action::ShowScripts,
        Action::ShowTerminal,
        Action::NewTerminalTab,
        Action::SplitRight,
        Action::MoveTabToOtherSide,
        Action::CloseSplit,
        Action::SelectNextTab,
        Action::SelectPreviousTab,
    ];

    /// The action's name, as menus show it.
    pub fn name(self) -> &'static str {
        match self {
            Action::NewProject => "New Project…",
            Action::Save => "Save",
            Action::CloseTab => "Close Tab",
            Action::OpenConfig => "Open Config",
            Action::ReloadEnvironment => "Reload Environment",
            Action::InstallDependencies => "Install Dependencies",
            Action::RestartLanguageServer => "Restart Language Server",
            Action::RunProjectCheck => "Run Project Check",
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
            Action::ShowQuickFixes => "Show Quick Fixes",
            Action::OrganizeImports => "Organize Imports",
            Action::ReformatFile => "Reformat File",
            Action::ExpandSelection => "Expand Selection",
            Action::ShrinkSelection => "Shrink Selection",
            Action::ExpandFold => "Expand Fold",
            Action::CollapseFold => "Collapse Fold",
            Action::ExpandAllFolds => "Expand All Folds",
            Action::CollapseAllFolds => "Collapse All Folds",
            Action::CodeCompletion => "Code Completion",
            Action::QuickDocumentation => "Quick Documentation",
            Action::ParameterInfo => "Parameter Info",
            Action::GoToFile => "Go to File…",
            Action::RecentFiles => "Recent Files",
            Action::FindAction => "Find Action…",
            Action::SearchEverywhere => "Search Everywhere",
            Action::FileStructure => "File Structure",
            Action::GoToSymbol => "Go to Symbol…",
            Action::ShowFiles => "Files",
            Action::ShowSearch => "Search",
            Action::ShowChanges => "Changes",
            Action::ShowProblems => "Problems",
            Action::ShowScripts => "Scripts",
            Action::ShowTerminal => "Terminal",
            Action::NewTerminalTab => "New Terminal Tab",
            Action::SplitRight => "Split Right",
            Action::MoveTabToOtherSide => "Move Tab to Other Side",
            Action::CloseSplit => "Close Split",
            Action::SelectNextTab => "Select Next Tab",
            Action::SelectPreviousTab => "Select Previous Tab",
            Action::GoToDefinition => "Go to Definition",
            Action::GoToTypeDefinition => "Go to Type Definition",
            Action::GoToImplementation => "Go to Implementation",
            Action::FindUsages => "Find Usages",
            Action::Rename => "Rename…",
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
            Action::ShowQuickFixes => "⌥↩",
            Action::OrganizeImports => "⌃⌥O",
            Action::ReformatFile => "⌥⌘L",
            Action::ExpandSelection => "⌥↑",
            Action::ShrinkSelection => "⌥↓",
            Action::ExpandFold => "⌥⌘=",
            Action::CollapseFold => "⌥⌘-",
            Action::CodeCompletion => "⌃Space",
            Action::QuickDocumentation => "F1",
            Action::ParameterInfo => "⌘P",
            Action::GoToFile => "⇧⌘O",
            Action::RecentFiles => "⌘E",
            Action::FindAction => "⇧⌘A",
            Action::SearchEverywhere => "⇧⇧",
            Action::FileStructure => "⌘F12",
            Action::GoToSymbol => "⌥⌘O",
            Action::ShowFiles => "⌘1",
            Action::ShowSearch => "⇧⌘F",
            Action::ShowProblems => "⌘6",
            Action::ShowTerminal => "⌥F12",
            Action::NewTerminalTab => "⌘T",
            Action::SelectNextTab => "⇧⌘]",
            Action::SelectPreviousTab => "⇧⌘[",
            Action::GoToDefinition => "⌘B",
            Action::GoToTypeDefinition => "⇧⌘B",
            Action::GoToImplementation => "⌥⌘B",
            Action::FindUsages => "⌥F7",
            Action::Rename => "⇧F6",
            Action::NewProject
            | Action::OpenConfig
            | Action::ReloadEnvironment
            | Action::InstallDependencies
            | Action::RestartLanguageServer
            | Action::RunProjectCheck
            | Action::SetRuntime
            | Action::SetPackageManager
            | Action::UpdateToolchain
            | Action::RemoveUnusedToolchains
            | Action::ExpandAllFolds
            | Action::CollapseAllFolds
            | Action::ShowScripts
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
                | Action::ShowQuickFixes
                | Action::OrganizeImports
                | Action::ReformatFile
                | Action::ExpandSelection
                | Action::ShrinkSelection
                | Action::ExpandFold
                | Action::CollapseFold
                | Action::ExpandAllFolds
                | Action::CollapseAllFolds
                | Action::GoToDefinition
                | Action::GoToTypeDefinition
                | Action::GoToImplementation
                | Action::FindUsages
                | Action::Rename
                | Action::CodeCompletion
                | Action::QuickDocumentation
                | Action::ParameterInfo
        )
    }

    /// The command the action runs, for actions that don't depend on
    /// which tab is active. `None` for the tab actions.
    pub(crate) fn command(self) -> Option<Command> {
        Some(match self {
            Action::NewProject => Command::NewProject,
            Action::Save => Command::Save,
            Action::OpenConfig => Command::OpenConfig,
            Action::ReloadEnvironment => Command::ReloadEnvironment,
            Action::InstallDependencies => Command::InstallDependencies,
            Action::RestartLanguageServer => Command::RestartLanguageServer,
            Action::RunProjectCheck => Command::RunProjectCheck,
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
            Action::ShowQuickFixes => Command::ShowQuickFixes,
            Action::OrganizeImports => Command::OrganizeImports,
            Action::ReformatFile => Command::ReformatFile,
            Action::ExpandSelection => Command::ExpandSelection,
            Action::ShrinkSelection => Command::ShrinkSelection,
            Action::ExpandFold => Command::ExpandFold,
            Action::CollapseFold => Command::CollapseFold,
            Action::ExpandAllFolds => Command::ExpandAllFolds,
            Action::CollapseAllFolds => Command::CollapseAllFolds,
            Action::CodeCompletion => Command::ShowCompletion,
            Action::QuickDocumentation => Command::ShowHover,
            Action::ParameterInfo => Command::ShowSignatureHelp,
            Action::GoToFile => Command::OpenFinder(FinderMode::Files),
            Action::RecentFiles => Command::OpenFinder(FinderMode::RecentFiles),
            Action::FindAction => Command::OpenFinder(FinderMode::Actions),
            Action::SearchEverywhere => Command::OpenFinder(FinderMode::Everywhere),
            Action::FileStructure => Command::OpenFinder(FinderMode::FileSymbols),
            Action::GoToSymbol => Command::OpenFinder(FinderMode::ProjectSymbols),
            Action::ShowFiles => Command::ToggleLeftColumn(LeftColumnView::Files),
            Action::ShowSearch => Command::ToggleLeftColumn(LeftColumnView::Search),
            Action::ShowChanges => Command::ToggleLeftColumn(LeftColumnView::Changes),
            Action::ShowProblems => Command::ToggleLeftColumn(LeftColumnView::Problems),
            Action::ShowScripts => Command::ToggleLeftColumn(LeftColumnView::Scripts),
            Action::ShowTerminal => Command::ToggleTerminal,
            Action::NewTerminalTab => Command::NewTerminalTab,
            Action::SplitRight => Command::SplitRight,
            Action::CloseSplit => Command::CloseSplit,
            Action::GoToDefinition => Command::GoToDefinition,
            Action::GoToTypeDefinition => Command::GoToTypeDefinition,
            Action::GoToImplementation => Command::GoToImplementation,
            Action::FindUsages => Command::FindUsages,
            Action::Rename => Command::StartRename,
            Action::CloseTab | Action::MoveTabToOtherSide | Action::SelectNextTab | Action::SelectPreviousTab => {
                return None;
            }
        })
    }
}
