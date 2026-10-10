//! A fake `tsc` for the **project check** (ticket #48): the same program
//! name as tsgo, so it plays both. A start with `--lsp` is the language
//! server (a [`FakeLsp`]); any other start is a command-line check, which
//! prints the scripted output (what `tsc -b --noEmit --pretty false`
//! prints) and exits with the scripted code.

use std::{
    io::Write,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use genea_host::ProcessSpec;

use crate::{FakeLsp, TestHost};

/// A scripted `tsc`. A cheap handle: install it, keep a clone to change
/// what the next check prints and to release a held one.
#[derive(Clone, Default)]
pub struct FakeTsc {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    output: String,
    code: i32,
    /// Checks wait until released.
    hold: bool,
    /// Checks that started, and whether each was killed before it ended.
    checks: Vec<(ProcessSpec, bool)>,
}

impl FakeTsc {
    /// A check that finds nothing: no output, exit code 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// What every check from now on prints on stdout, and its exit code.
    pub fn reports(self, output: &str, code: i32) -> Self {
        self.set_report(output, code);
        self
    }

    /// Changes what the next check prints, and its exit code.
    pub fn set_report(&self, output: &str, code: i32) {
        let mut state = self.inner.state.lock().unwrap();
        state.output = output.to_owned();
        state.code = code;
    }

    /// Checks wait (real time) until [`release`](Self::release)d or
    /// killed, so a test can look at a check while it runs.
    pub fn hold(self) -> Self {
        self.inner.state.lock().unwrap().hold = true;
        self
    }

    /// Lets held checks finish, and later ones run straight through.
    pub fn release(&self) {
        self.inner.state.lock().unwrap().hold = false;
        self.inner.changed.notify_all();
    }

    /// Every check started so far.
    pub fn checks(&self) -> Vec<ProcessSpec> {
        self.inner.state.lock().unwrap().checks.iter().map(|(spec, _)| spec.clone()).collect()
    }

    /// How many checks were killed before they finished.
    pub fn killed(&self) -> usize {
        self.inner.state.lock().unwrap().checks.iter().filter(|(_, killed)| *killed).count()
    }

    /// Plays every start of `program` (`tsc`) on the test host: `--lsp`
    /// starts go to `lsp`, the rest are checks.
    pub fn install(&self, host: &TestHost, program: &str, lsp: &FakeLsp) {
        let (fake, lsp) = (self.clone(), lsp.clone());
        host.processes().script(program, move |spec, mut io| {
            if spec.args.iter().any(|arg| arg == "--lsp") {
                return lsp.run(io);
            }
            let inner = &fake.inner;
            let mut state = inner.state.lock().unwrap();
            let index = state.checks.len();
            state.checks.push((spec.clone(), false));
            while state.hold && !io.killed() {
                state = inner.changed.wait_timeout(state, Duration::from_millis(10)).unwrap().0;
            }
            if io.killed() {
                state.checks[index].1 = true;
                return 137;
            }
            let (output, code) = (state.output.clone(), state.code);
            drop(state);
            let _ = io.stdout.write_all(output.as_bytes());
            code
        });
    }
}
