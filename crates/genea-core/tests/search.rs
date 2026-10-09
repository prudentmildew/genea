//! Project search (⌘⇧F, ticket #34): results grouped by file, in literal,
//! regex, case-sensitive and whole-word modes, without ignored or excluded
//! files; a click opens the file at the match.

use genea_core::{Command, LeftColumnView, MAX_SEARCH_MATCHES, ProjectId, SearchQuery, Workbench};
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
fn the_search_shortcut_shows_the_search_view_or_collapses_the_column() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new());
    let left_column = |workbench: &Workbench| workbench.project(project).unwrap().left_column;

    workbench.dispatch(project, Command::ToggleLeftColumn(LeftColumnView::Search));
    assert_eq!(left_column(&workbench), Some(LeftColumnView::Search));

    workbench.dispatch(project, Command::ToggleLeftColumn(LeftColumnView::Search));
    assert_eq!(left_column(&workbench), None);
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

#[test]
fn a_literal_search_matches_regex_characters_as_they_are() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "f(x);\nfx;\nf(y);\n"));

    search(&mut workbench, project, literal("f(x)"));

    assert_eq!(results(&workbench, project), ["a.ts", "  1:1 [f(x)];"]);
}

#[test]
fn a_regex_search_matches_the_pattern() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "let a1 = 1;\nlet b = 22;\nlet c = x;\n"));

    search(&mut workbench, project, SearchQuery { regex: true, ..literal(r"= \d+") });

    assert_eq!(results(&workbench, project), ["a.ts", "  1:8 let a1 [= 1];", "  2:7 let b [= 22];"]);
}

#[test]
fn an_invalid_regex_shows_an_error_and_no_results() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "f(x);\n"));

    search(&mut workbench, project, SearchQuery { regex: true, ..literal("f(") });

    let view = workbench.project(project).unwrap().search;
    assert!(view.error.is_some_and(|error| error.contains("unclosed group")));
    assert!(view.files.is_empty());
    assert!(!view.searching);
}

#[test]
fn a_case_sensitive_search_tells_upper_and_lower_case_apart() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "type Total = number;\nconst total: Total = 0;\n"));

    search(&mut workbench, project, SearchQuery { case_sensitive: true, ..literal("Total") });

    assert_eq!(results(&workbench, project), ["a.ts", "  1:6 type [Total] = number;", "  2:14 const total: [Total] = 0;"]);
}

#[test]
fn a_whole_word_search_skips_matches_inside_words() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "const id = getId(id);\nconst identity = id_2;\n"));

    search(&mut workbench, project, SearchQuery { whole_word: true, ..literal("id") });

    assert_eq!(results(&workbench, project), ["a.ts", "  1:7 const [id] = getId(id);", "  1:18 const id = getId([id]);"]);
}

#[test]
fn whole_word_and_case_combine_with_a_regex() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "fooBar foo_bar FOO foo\n"));

    let query = SearchQuery { regex: true, case_sensitive: true, whole_word: true, ..literal("fo+") };
    search(&mut workbench, project, query);

    assert_eq!(results(&workbench, project), ["a.ts", "  1:20 fooBar foo_bar FOO [foo]"]);
}

#[test]
fn ignored_excluded_and_node_modules_files_never_appear_in_results() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file(".gitignore", "dist/\n*.log\n")
            .file("genea.jsonc", r#"{ "exclude": ["fixtures/", "*.snap"] }"#)
            .file("src/a.ts", "needle\n")
            .file(".env", "NEEDLE=1\n")
            .file("dist/a.js", "needle\n")
            .file("debug.log", "needle\n")
            .file("packages/p/.gitignore", "gen/\n")
            .file("packages/p/gen/b.ts", "needle\n")
            .file("packages/p/src/b.ts", "needle\n")
            .file("fixtures/c.ts", "needle\n")
            .file("src/a.test.ts.snap", "needle\n")
            .file("node_modules/left-pad/index.js", "needle\n")
            .file("packages/p/node_modules/x/index.js", "needle\n")
            .file(".git/config", "needle\n"),
    );

    search(&mut workbench, project, literal("needle"));

    assert_eq!(
        results(&workbench, project),
        [".env", "  1:1 [NEEDLE]=1", "packages/p/src/b.ts", "  1:1 [needle]", "src/a.ts", "  1:1 [needle]"]
    );
}

#[test]
fn binary_files_are_skipped() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "needle\n").file("image.png", b"\x89PNG\0\0needle\n"));

    search(&mut workbench, project, literal("needle"));

    assert_eq!(results(&workbench, project), ["a.ts", "  1:1 [needle]"]);
}

#[test]
fn a_new_query_replaces_the_search_in_flight() {
    let mut fixture = FixtureProject::new();
    for i in 0..200 {
        fixture = fixture.file(format!("src/f{i:03}.ts"), "alpha\n".repeat(50));
    }
    fixture = fixture.file("src/zeta.ts", "beta\n");
    let (_fixture, mut workbench, project) = open(fixture);

    workbench.dispatch(project, Command::Search(literal("alpha")));
    workbench.dispatch(project, Command::Search(literal("beta")));
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap().search;
    assert_eq!(view.query, literal("beta"));
    assert_eq!(results(&workbench, project), ["src/zeta.ts", "  1:1 [beta]"]);
    assert_eq!(view.match_count, 1);
}

#[test]
fn an_empty_query_clears_the_results() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "needle\n"));
    search(&mut workbench, project, literal("needle"));

    search(&mut workbench, project, literal(""));

    let view = workbench.project(project).unwrap().search;
    assert!(view.files.is_empty());
    assert_eq!(view.match_count, 0);
    assert!(!view.searching);
}

#[test]
fn clicking_a_result_opens_the_file_with_the_caret_at_the_match() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new().file("src/a.ts", "import x from 'y';\n\nexport const total = x + 1;\n").file("b.ts", ""),
    );
    workbench.dispatch(project, Command::OpenFile("b.ts".into()));
    search(&mut workbench, project, literal("total"));

    let found = workbench.project(project).unwrap().search.files[0].clone();
    workbench.dispatch(project, Command::OpenFileAt { path: found.path, at: found.matches[0].position });
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    let editor = view.editor.unwrap();
    assert_eq!(editor.path, std::path::Path::new("src/a.ts"));
    assert_eq!(view.status.caret.as_deref(), Some("3:14"));
}

#[test]
fn long_lines_are_cut_short_around_the_match() {
    let line = format!("{}needle{}", "a".repeat(100), "b".repeat(200));
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("min.js", &line));

    search(&mut workbench, project, literal("needle"));

    let m = workbench.project(project).unwrap().search.files[0].matches[0].clone();
    assert_eq!(m.location, "1:101");
    assert_eq!(m.before, format!("…{}", "a".repeat(24)));
    assert_eq!(m.after, format!("{}…", "b".repeat(120)));
}

#[test]
fn a_search_stops_at_the_match_limit_and_says_so() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file("a.ts", "x\n".repeat(MAX_SEARCH_MATCHES - 1))
            .file("b.ts", "x x\n")
            .file("c.ts", "x\n"),
    );

    search(&mut workbench, project, literal("x"));

    let view = workbench.project(project).unwrap().search;
    assert_eq!(view.match_count, MAX_SEARCH_MATCHES);
    assert!(view.limited);
    assert_eq!(view.files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(), ["a.ts", "b.ts"]);

    search(&mut workbench, project, literal("x x"));
    assert!(!workbench.project(project).unwrap().search.limited);
}
