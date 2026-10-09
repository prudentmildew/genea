//! Project search (⌘⇧F, ticket #34): results grouped by file, in literal,
//! regex, case-sensitive and whole-word modes, without ignored or excluded
//! files; a click opens the file at the match.

use genea_core::{Command, ProjectId, SearchQuery, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

fn open(fixture: FixtureBuilder) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn search(workbench: &mut Workbench, project: ProjectId, query: SearchQuery) {
    workbench.dispatch(project, Command::Search(query));
    workbench.settle().unwrap();
}

fn literal(text: &str) -> SearchQuery {
    SearchQuery { text: text.into(), ..SearchQuery::default() }
}

/// The results as the user reads them: a line per file, then a line per
/// match with its `line:column` and the line's text, the match in brackets.
fn results(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let view = workbench.project(project).unwrap().search;
    let mut lines = Vec::new();
    for file in view.files.iter() {
        lines.push(file.path.display().to_string());
        for m in &file.matches {
            lines.push(format!("  {} {}[{}]{}", m.location, m.before, m.matched, m.after));
        }
    }
    lines
}

#[test]
fn a_literal_search_lists_every_match_grouped_by_file() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file("src/a.ts", "const total = 1;\nlet sum = total + total;\n")
            .file("src/b.ts", "// nothing here\n")
            .file("README.md", "Total: none\n"),
    );

    search(&mut workbench, project, literal("total"));

    assert_eq!(
        results(&workbench, project),
        [
            "README.md",
            "  1:1 [Total]: none",
            "src/a.ts",
            "  1:7 const [total] = 1;",
            "  2:11 let sum = [total] + total;",
            "  2:19 let sum = total + [total];",
        ]
    );
    let view = workbench.project(project).unwrap().search;
    assert!(!view.searching);
    assert_eq!(view.match_count, 4);
}
