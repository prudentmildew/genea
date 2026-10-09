//! The terminal (ticket #38): one shell per project, in a pane next to the
//! editor, running on a PTY through the host.
//!
//! The test host's fake PTY plays the shell: `FakePty::output` replays
//! what the shell prints and returns once Genea has read it, so a
//! `settle()` after it shows it on the grid; `wait_for_input` sees what the
//! user typed.

use std::{path::Path, sync::mpsc, time::Duration};

use genea_core::{
    Command, Modifiers, MouseAction, MouseButton, ProjectId, TerminalColor, TerminalKey, TerminalRun, TerminalStatus, TerminalStyle, TerminalView,
    ToolState, ToolView, Workbench,
};
use genea_host::PtySize;
use genea_testkit::{FakePty, FixtureProject, TestHost};

/// A project open on `host`, settled, with its terminal's fake PTY.
fn open(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId, FakePty) {
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let pty = host.ptys().last();
    (workbench, project, pty)
}

fn fixture() -> FixtureProject {
    FixtureProject::new().file("src/main.ts", "").build()
}

fn terminal(workbench: &Workbench, project: ProjectId) -> TerminalView {
    workbench.project(project).unwrap().terminal
}

/// The grid's rows as text, without the empty rows at the bottom.
fn screen(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let mut lines: Vec<String> = terminal(workbench, project).lines.into_iter().map(|line| line.text).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn var(pty: &FakePty, key: &str) -> Option<String> {
    let env = pty.spec().env;
    env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.to_string_lossy().into_owned())
}

#[test]
fn the_shell_is_the_users_login_shell_in_the_project_root() {
    let host = TestHost::new();
    host.set_launch_environment([("SHELL", "/bin/bash"), ("PATH", "/usr/bin:/bin")]);
    host.processes().script_shell("bash", [("PATH", "/opt/mine/bin:/usr/bin:/bin"), ("GREETING", "hello")]);
    let fixture = fixture();

    let (_workbench, _project, pty) = open(&host, &fixture);

    let spec = pty.spec();
    assert_eq!(spec.program, Path::new("/bin/bash"));
    assert_eq!(spec.args, ["-l"]);
    assert_eq!(spec.cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().as_path()));
    assert!(spec.clear_env);
    // The project environment: the login shell's variables.
    assert_eq!(var(&pty, "GREETING").as_deref(), Some("hello"));
    assert_eq!(var(&pty, "PATH").as_deref(), Some("/opt/mine/bin:/usr/bin:/bin"));
    assert_eq!(var(&pty, "TERM").as_deref(), Some("xterm-256color"));
    assert_eq!(var(&pty, "COLORTERM").as_deref(), Some("truecolor"));
}

#[test]
fn what_the_shell_prints_shows_on_the_grid() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);

    pty.output("hello\r\nworld");
    workbench.settle().unwrap();

    assert_eq!(screen(&workbench, project), ["hello", "world"]);
    let cursor = terminal(&workbench, project).cursor.unwrap();
    assert_eq!((cursor.line, cursor.column), (1, 5));
}

/// Plays the shell for one command line: runs it with `/bin/sh` in the
/// terminal's environment and folder, and prints its output on the PTY.
fn shell_runs(pty: &FakePty, command: &str) {
    let spec = pty.spec();
    let mut sh = std::process::Command::new("/bin/sh");
    sh.arg("-c").arg(command).env_clear().envs(spec.env.iter().map(|(k, v)| (k, v)));
    if let Some(cwd) = &spec.cwd {
        sh.current_dir(cwd);
    }
    let out = sh.output().unwrap();
    pty.output(String::from_utf8_lossy(&out.stdout).replace('\n', "\r\n"));
}

/// Types a command line into the terminal, as the user would.
fn type_line(workbench: &mut Workbench, project: ProjectId, line: &str) {
    workbench.dispatch(project, Command::TerminalText(line.into()));
    workbench.dispatch(project, Command::TerminalKey(TerminalKey::Enter, Modifiers::default()));
}

#[test]
fn node_and_pnpm_in_the_terminal_are_the_pinned_versions_once_downloaded() {
    let host = TestHost::new();
    let node = host.tools().node("24.18.0");
    host.tools().pnpm("11.13.0");
    host.download_server().hold(&node);
    let package_json = r#"{
  "name": "app",
  "devEngines": { "runtime": { "name": "node", "version": "24.18.0" } },
  "packageManager": "pnpm@11.13.0"
}
"#;
    let fixture = FixtureProject::new().file("package.json", package_json).build();
    let mut workbench = Workbench::new(host.shared());
    let (notify, notified) = mpsc::channel();
    workbench.set_notifier(move || {
        let _ = notify.send(());
    });
    let project = workbench.open_project(fixture.root()).unwrap();

    // The shell waits for the pinned tools, so they are on its PATH.
    loop {
        workbench.pump();
        let runtime = workbench.project(project).unwrap().toolchain.runtime;
        if matches!(runtime, Some(ToolView { state: ToolState::Downloading { .. }, .. })) {
            break;
        }
        notified.recv_timeout(Duration::from_secs(10)).expect("Node never started downloading");
    }
    host.download_server().wait_held(&node);
    workbench.pump();
    assert!(host.ptys().spawned().is_empty());
    assert_eq!(terminal(&workbench, project).status, TerminalStatus::Starting);

    host.download_server().release(&node);
    workbench.settle().unwrap();
    let pty = host.ptys().last();
    assert_eq!(terminal(&workbench, project).status, TerminalStatus::Running);

    type_line(&mut workbench, project, "node -v");
    pty.wait_for_input("node -v\r");
    shell_runs(&pty, "node -v");
    type_line(&mut workbench, project, "pnpm -v");
    pty.wait_for_input("pnpm -v\r");
    shell_runs(&pty, "pnpm -v");
    workbench.settle().unwrap();

    assert_eq!(screen(&workbench, project), ["v24.18.0", "11.13.0"]);
}

#[test]
fn keys_are_sent_the_way_xterm_sends_them_in_the_programs_mode() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    let (ctrl, alt, shift) = (
        Modifiers { ctrl: true, ..Modifiers::default() },
        Modifiers { alt: true, ..Modifiers::default() },
        Modifiers { shift: true, ..Modifiers::default() },
    );

    for (key, modifiers) in [
        (TerminalKey::Up, Modifiers::default()),
        (TerminalKey::Char('c'), ctrl),
        (TerminalKey::Left, alt),
        (TerminalKey::Right, shift),
        (TerminalKey::Backspace, Modifiers::default()),
        (TerminalKey::F(5), Modifiers::default()),
    ] {
        workbench.dispatch(project, Command::TerminalKey(key, modifiers));
    }
    assert_eq!(pty.wait_for_input("\x1b[15~"), "\x1b[A\x03\x1bb\x1b[1;2C\x7f\x1b[15~");

    // A full-screen program turns on application cursor keys.
    pty.output("\x1b[?1h");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::TerminalKey(TerminalKey::Up, Modifiers::default()));
    assert!(pty.wait_for_input("\x1bOA").ends_with("\x1b[15~\x1bOA"));
}

#[test]
fn the_terminal_takes_the_size_the_view_gives_it() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    assert_eq!(pty.size(), PtySize { rows: 24, columns: 80 });

    workbench.dispatch(project, Command::SetTerminalSize { rows: 5, columns: 3 });
    pty.wait_for_size(PtySize { rows: 5, columns: 3 });
    pty.output("abcdef");
    workbench.settle().unwrap();

    let view = terminal(&workbench, project);
    assert_eq!((view.rows, view.columns, view.lines.len()), (5, 3, 5));
    // Lines wrap at the new width.
    assert_eq!(screen(&workbench, project), ["abc", "def"]);
}

#[test]
fn a_shell_started_after_a_resize_gets_the_new_size() {
    let host = TestHost::new();
    // The login shell never answers, so the terminal waits for it.
    host.set_launch_environment([("SHELL", "/bin/zsh"), ("PATH", "/usr/bin:/bin")]);
    host.processes().script("zsh", |_, io| {
        while !io.killed() {
            std::thread::sleep(Duration::from_millis(1));
        }
        1
    });
    let fixture = fixture();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();

    workbench.dispatch(project, Command::SetTerminalSize { rows: 40, columns: 120 });
    host.clock().advance(genea_core::LOGIN_SHELL_TIMEOUT);
    workbench.settle().unwrap();

    assert_eq!(host.ptys().last().size(), PtySize { rows: 40, columns: 120 });
}

fn run(columns: std::ops::Range<usize>, text: &str, style: TerminalStyle) -> TerminalRun {
    TerminalRun { columns, text: text.into(), style }
}

#[test]
fn colours_and_attributes_come_with_the_text() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);

    // Bold red (ANSI), a 24-bit foreground on a 256-colour background, and
    // inverse video.
    pty.output("\x1b[1;31mred\x1b[0m \x1b[38;2;10;20;30;48;5;196mrgb\x1b[0m \x1b[7minv\x1b[0m plain");
    workbench.settle().unwrap();

    let line = &terminal(&workbench, project).lines[0];
    assert_eq!(line.text, "red rgb inv plain");
    let plain = TerminalStyle::default();
    assert_eq!(
        line.runs,
        [
            run(0..3, "red", TerminalStyle { foreground: TerminalColor::Ansi(1), bold: true, ..plain.clone() }),
            run(3..4, " ", plain.clone()),
            run(
                4..7,
                "rgb",
                TerminalStyle {
                    foreground: TerminalColor::Rgb(10, 20, 30),
                    background: TerminalColor::Rgb(255, 0, 0),
                    ..plain.clone()
                }
            ),
            run(7..8, " ", plain.clone()),
            run(
                8..11,
                "inv",
                TerminalStyle {
                    foreground: TerminalColor::Background,
                    background: TerminalColor::Foreground,
                    ..plain.clone()
                }
            ),
            run(11..17, " plain", plain.clone()),
        ]
    );
}

#[test]
fn wide_characters_take_two_columns() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);

    pty.output("日本 ok");
    workbench.settle().unwrap();

    let view = terminal(&workbench, project);
    assert_eq!(view.lines[0].text, "日本 ok");
    assert_eq!(view.lines[0].runs, [run(0..7, "日本 ok", TerminalStyle::default())]);
    assert_eq!(view.cursor.unwrap().column, 7);
}

#[test]
fn the_scrollback_keeps_ten_thousand_lines() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    let output: String = (0..10_100).map(|n| format!("line {n}\r\n")).collect();
    pty.output(output);
    workbench.settle().unwrap();
    assert_eq!(screen(&workbench, project).last().unwrap(), "line 10099");

    workbench.dispatch(project, Command::ScrollTerminal { rows: -100_000, line: 0, column: 0 });

    let view = terminal(&workbench, project);
    assert_eq!(view.history, 10_000);
    assert_eq!(view.scrolled_back, 10_000);
    // 24 rows: the last holds the prompt's empty line, so the screen began
    // at line 10077 and the scrollback 10,000 lines before it.
    assert_eq!(view.lines[0].text, "line 77");
    assert_eq!(view.cursor, None);

    workbench.dispatch(project, Command::ScrollTerminal { rows: 9_990, line: 0, column: 0 });
    assert_eq!(terminal(&workbench, project).lines[0].text, "line 10067");

    // Typing goes back to the bottom.
    workbench.dispatch(project, Command::TerminalText("x".into()));
    let view = terminal(&workbench, project);
    assert_eq!(view.scrolled_back, 0);
    assert_eq!(view.lines[22].text, "line 10099");
}

/// A full-screen program's output: the alternate screen, a hidden cursor,
/// SGR mouse reporting, and a 24-bit coloured title bar over a list.
const TUI_START: &str = concat!(
    "\x1b[?1049h", // alternate screen
    "\x1b[?25l",   // hide the cursor
    "\x1b[?1000h\x1b[?1002h\x1b[?1006h", // clicks, drags, SGR encoding
    "\x1b[2J\x1b[H",
    "\x1b[48;2;40;44;52m\x1b[38;2;220;220;220m Files \x1b[0m",
    "\x1b[3;3H> main.ts",
    "\x1b[4;3H  util.ts",
);
const TUI_END: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l";

fn mouse(action: MouseAction, line: usize, column: usize) -> Command {
    Command::TerminalMouse { action, line, column, modifiers: Modifiers::default() }
}

#[test]
fn a_full_screen_program_draws_on_the_alternate_screen_and_takes_the_mouse() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    pty.output("~/app $ tui\r\n");
    workbench.settle().unwrap();
    // Without mouse reporting, clicks don't reach the program.
    workbench.dispatch(project, mouse(MouseAction::Press(MouseButton::Left), 0, 0));

    pty.output(TUI_START);
    workbench.settle().unwrap();

    let view = terminal(&workbench, project);
    assert!(view.alternate_screen && view.mouse_reporting);
    assert_eq!(view.cursor, None);
    assert_eq!(screen(&workbench, project), [" Files", "", "  > main.ts", "    util.ts"]);
    let bar = TerminalStyle {
        foreground: TerminalColor::Rgb(220, 220, 220),
        background: TerminalColor::Rgb(40, 44, 52),
        ..TerminalStyle::default()
    };
    assert_eq!(view.lines[0].runs, [run(0..7, " Files ", bar)]);

    // A click on util.ts, a drag, the wheel; then a key.
    workbench.dispatch(project, mouse(MouseAction::Press(MouseButton::Left), 3, 4));
    workbench.dispatch(project, mouse(MouseAction::Drag(MouseButton::Left), 3, 6));
    workbench.dispatch(project, mouse(MouseAction::Release(MouseButton::Left), 3, 6));
    workbench.dispatch(project, Command::ScrollTerminal { rows: -2, line: 2, column: 0 });
    workbench.dispatch(project, Command::TerminalText("q".into()));
    assert_eq!(
        pty.wait_for_input("q"),
        "\x1b[<0;5;4M\x1b[<32;7;4M\x1b[<0;7;4m\x1b[<64;1;3M\x1b[<64;1;3Mq"
    );

    // It exits: the shell's screen is back, with its scrollback.
    pty.output(TUI_END);
    workbench.settle().unwrap();
    let view = terminal(&workbench, project);
    assert!(!view.alternate_screen && !view.mouse_reporting);
    assert_eq!(screen(&workbench, project), ["~/app $ tui"]);
    assert!(view.cursor.is_some());
}

#[test]
fn the_wheel_in_a_full_screen_program_without_the_mouse_sends_arrows() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    pty.output("\x1b[?1049h\x1b[?1h");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::ScrollTerminal { rows: -2, line: 0, column: 0 });
    workbench.dispatch(project, Command::ScrollTerminal { rows: 1, line: 0, column: 0 });

    assert_eq!(pty.wait_for_input("\x1bOB"), "\x1bOA\x1bOA\x1bOB");
}

#[test]
fn mouse_reports_use_the_programs_encoding() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    // Clicks only, in xterm's original encoding.
    pty.output("\x1b[?1000h");
    workbench.settle().unwrap();

    let shift = Modifiers { shift: true, ..Modifiers::default() };
    workbench.dispatch(project, Command::TerminalMouse {
        action: MouseAction::Press(MouseButton::Right),
        line: 1,
        column: 2,
        modifiers: shift,
    });
    // Drags aren't reported in this mode.
    workbench.dispatch(project, mouse(MouseAction::Drag(MouseButton::Right), 1, 3));
    workbench.dispatch(project, mouse(MouseAction::Release(MouseButton::Right), 1, 3));

    assert_eq!(pty.wait_for_input("\x1b[M#"), "\x1b[M&#\"\x1b[M#$\"");
}

#[test]
fn a_paste_is_bracketed_when_the_program_asks() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    host.clipboard().set_text("echo a\necho b");

    // Without bracketed paste, line breaks are Returns.
    workbench.dispatch(project, Command::TerminalPaste);
    pty.wait_for_input("echo b");
    pty.output("\x1b[?2004h");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::TerminalPaste);

    assert_eq!(pty.wait_for_input("\x1b[201~"), "echo a\recho b\x1b[200~echo a\necho b\x1b[201~");
}

#[test]
fn hyperlinks_and_the_title_come_from_the_program() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);
    // Until the program sets one, the title is the shell's name.
    assert_eq!(terminal(&workbench, project).title, "zsh");

    pty.output("\x1b]0;vim main.ts\x07see \x1b]8;;https://genea.dev/docs\x1b\\the docs\x1b]8;;\x1b\\.");
    workbench.settle().unwrap();

    let view = terminal(&workbench, project);
    assert_eq!(view.title, "vim main.ts");
    assert_eq!(view.lines[0].text, "see the docs.");
    let link = TerminalStyle { link: Some("https://genea.dev/docs".into()), ..TerminalStyle::default() };
    assert_eq!(view.lines[0].runs[1], run(4..12, "the docs", link));
}

#[test]
fn a_program_can_copy_to_the_clipboard() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, _project, pty) = open(&host, &fixture);

    // OSC 52 with "copied" in base64.
    pty.output("\x1b]52;c;Y29waWVk\x07");
    workbench.settle().unwrap();

    assert_eq!(host.clipboard().text().as_deref(), Some("copied"));
}
