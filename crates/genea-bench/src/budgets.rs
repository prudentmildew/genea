//! The budgets the harness checks, and the verdict over a run.
//!
//! A [`Budget`] is a limit on one measured figure. A *Genea budget*
//! ([`Kind::Budget`], GLOSSARY.md) fails the run when it is missed; an
//! *end-to-end target* ([`Kind::Target`]) is reported but never fails it.
//! Every limit is an upper bound ("at most"), so a functional check (the
//! dead-key composition) is a budget of zero mismatches.
//!
//! The table below is #6 as revised by #18 (spec #19, Benchmark harness).
//! #63 adds its rows (navigation, memory with workspaces open, end-to-end
//! targets) here.

use serde_json::{Value, json};

use crate::stats::round;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A Genea budget: missing it fails the run.
    Budget,
    /// An end-to-end target: reported only.
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// Stable id for the JSONL results, `scenario.figure`.
    pub id: &'static str,
    pub title: &'static str,
    pub limit: f64,
    pub unit: &'static str,
    pub kind: Kind,
}

// Start (each a margin over the start floor measured in the same session).
pub const WARM_START: Budget = Budget {
    id: "start.warm.margin",
    title: "Warm start: content visible over the start floor (p95)",
    limit: 50.0,
    unit: "ms",
    kind: Kind::Budget,
};
pub const COLD_START: Budget = Budget {
    id: "start.cold.margin",
    title: "Cold start after purge: content visible over the start floor (p95)",
    limit: 100.0,
    unit: "ms",
    kind: Kind::Budget,
};
// Input and rendering.
pub const KEYSTROKE: Budget = Budget {
    id: "typing.keystroke_to_frame",
    title: "Keystroke to frame, Genea's work (p95)",
    limit: 8.0,
    unit: "ms",
    kind: Kind::Budget,
};
pub const TYPING_STALL: Budget = Budget {
    id: "typing.stall",
    title: "Typing: longest main-thread stall",
    limit: 16.0,
    unit: "ms",
    kind: Kind::Budget,
};
pub const SCROLL_DROPPED: Budget = Budget {
    id: "scroll.dropped",
    title: "Scrolling: dropped frames at the display's rate",
    limit: 1.0,
    unit: "%",
    kind: Kind::Budget,
};
pub const SCROLL_FRAME_WORK: Budget = Budget {
    id: "scroll.frame_work",
    title: "Scrolling: frame work fits a 120 Hz frame (p95)",
    limit: 1000.0 / 120.0,
    unit: "ms",
    kind: Kind::Budget,
};
pub const SCROLL_STALL: Budget = Budget {
    id: "scroll.stall",
    title: "Scrolling: longest main-thread stall",
    limit: 16.0,
    unit: "ms",
    kind: Kind::Budget,
};
// Idle.
pub const IDLE_MEMORY: Budget = Budget {
    id: "idle.memory",
    title: "Idle memory, one file open, 1200×800 pt at 2× (p95)",
    limit: 100.0,
    unit: "MB",
    kind: Kind::Budget,
};
pub const IDLE_CPU: Budget = Budget {
    id: "idle.cpu",
    title: "Idle CPU (≈ 0 %; worst run)",
    limit: 0.1,
    unit: "%",
    kind: Kind::Budget,
};
// The IME bridge, end to end.
pub const DEAD_KEYS: Budget = Budget {
    id: "dead_keys.mismatches",
    title: "Dead-key composition through the IME bridge",
    limit: 0.0,
    unit: "wrong compositions",
    kind: Kind::Budget,
};
pub const DEAD_KEYS_STALL: Budget = Budget {
    id: "dead_keys.stall",
    title: "Dead keys: longest main-thread stall",
    limit: 16.0,
    unit: "ms",
    kind: Kind::Budget,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// A Genea budget missed: the run fails.
    Fail,
    /// An end-to-end target missed: reported only.
    Missed,
    /// Not measured, with a reason (no `sudo` for `purge`, say).
    Skipped,
}

/// A budget with what the run measured for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub budget: Budget,
    pub measured: Option<f64>,
    /// Why it was skipped, or context for the measurement.
    pub note: Option<String>,
}

impl Budget {
    /// `None` (nothing measured) is skipped.
    pub fn check(&self, measured: Option<f64>) -> Check {
        Check { budget: *self, measured, note: measured.is_none().then(|| "not measured".to_string()) }
    }

    pub fn skipped(&self, reason: &str) -> Check {
        Check { budget: *self, measured: None, note: Some(reason.to_string()) }
    }
}

impl Check {
    pub fn with_note(mut self, note: impl Into<String>) -> Check {
        self.note = Some(note.into());
        self
    }

    pub fn verdict(&self) -> Verdict {
        match (self.measured, self.budget.kind) {
            (None, _) => Verdict::Skipped,
            (Some(m), _) if m <= self.budget.limit => Verdict::Pass,
            (Some(_), Kind::Budget) => Verdict::Fail,
            (Some(_), Kind::Target) => Verdict::Missed,
        }
    }

    pub fn to_json(&self) -> Value {
        let verdict = match self.verdict() {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Missed => "missed",
            Verdict::Skipped => "skipped",
        };
        json!({
            "type": "check",
            "id": self.budget.id,
            "title": self.budget.title,
            "kind": match self.budget.kind { Kind::Budget => "budget", Kind::Target => "target" },
            "measured": self.measured.map(round),
            "limit": round(self.budget.limit),
            "unit": self.budget.unit,
            "verdict": verdict,
            "note": self.note,
        })
    }

    /// One summary line.
    pub fn line(&self) -> String {
        let b = &self.budget;
        let unit = |v: f64| if b.unit == "%" { format!("{} %", round(v)) } else { format!("{} {}", round(v), b.unit) };
        let mut line = match (self.verdict(), self.measured) {
            (Verdict::Skipped, _) | (_, None) => {
                format!("SKIP  {}: {}", b.title, self.note.as_deref().unwrap_or("not measured"))
            }
            (Verdict::Missed, Some(m)) => format!("MISS  {}: {} (target {}, report only)", b.title, unit(m), unit(b.limit)),
            (verdict, Some(m)) => {
                let tag = if verdict == Verdict::Pass { "PASS" } else { "FAIL" };
                let limit = if b.kind == Kind::Target { "target" } else { "limit" };
                format!("{tag}  {}: {} ({limit} {})", b.title, unit(m), unit(b.limit))
            }
        };
        if self.measured.is_some()
            && let Some(note) = &self.note
        {
            line.push_str(&format!(" [{note}]"));
        }
        line
    }
}

/// The verdict over a whole run.
pub struct Verdicts {
    pub lines: Vec<String>,
    /// Genea budgets checked, and how many of them were missed.
    pub budgets: usize,
    pub failed: usize,
    pub skipped: usize,
}

pub fn verdicts(checks: &[Check]) -> Verdicts {
    let count = |v: Verdict| checks.iter().filter(|c| c.verdict() == v).count();
    Verdicts {
        lines: checks.iter().map(Check::line).collect(),
        budgets: checks.iter().filter(|c| c.budget.kind == Kind::Budget).count(),
        failed: count(Verdict::Fail),
        skipped: count(Verdict::Skipped),
    }
}

impl Verdicts {
    pub fn passed(&self) -> bool {
        self.failed == 0
    }

    /// Non-zero when a Genea budget was missed.
    pub fn exit_code(&self) -> i32 {
        if self.passed() { 0 } else { 1 }
    }

    /// The pass/fail summary: one line per check, then the verdict.
    pub fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        let skipped = if self.skipped > 0 { format!(", {} skipped", self.skipped) } else { String::new() };
        let verdict = if self.passed() {
            format!("PASSED: {} Genea budgets met{skipped}", self.budgets)
        } else {
            format!("FAILED: {} of {} Genea budgets missed{skipped}", self.failed, self.budgets)
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&verdict);
        text.push('\n');
        text
    }
}
