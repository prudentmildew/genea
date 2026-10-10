use std::io::{self, Read, Write};

use crate::{Exit, ProcessSpec};

/// Starting programs on pseudo-terminals: the terminal's shells (ticket #38).
pub trait Ptys: Send + Sync {
    /// Starts `spec` on a new pseudo-terminal of `size`, as the leader of a
    /// new session: the terminal is its stdin, stdout, stderr and
    /// controlling terminal. `spec` is read as for [`Processes`].
    ///
    /// [`Processes`]: crate::Processes
    fn spawn(&self, spec: &ProcessSpec, size: PtySize) -> io::Result<Pty>;
}

/// A terminal's size in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub columns: u16,
}

/// A program running on a pseudo-terminal. Read its output on one thread
/// and write its input on another; the control works from any thread.
pub struct Pty {
    /// What the program writes to its terminal. End of file (or an error)
    /// once the terminal is closed on the program's side.
    pub output: Box<dyn Read + Send>,
    /// What the user types into the terminal.
    pub input: Box<dyn Write + Send>,
    pub control: Box<dyn PtyControl>,
}

/// Resizing, signalling and waiting for a program on a pseudo-terminal.
pub trait PtyControl: Send + Sync {
    /// The OS process id, if there is a real process.
    fn id(&self) -> Option<u32>;
    /// Tells the terminal (and so the program, with SIGWINCH) its new size.
    fn resize(&self, size: PtySize) -> io::Result<()>;
    /// Hangs up the terminal, as closing a terminal window does: the
    /// program gets SIGHUP (a shell passes it on to its jobs).
    fn hang_up(&self) -> io::Result<()>;
    /// Interrupts the program, as ⌃C does: its process group gets SIGINT
    /// (ticket #40's Stop).
    fn interrupt(&self) -> io::Result<()>;
    /// Kills the program and its process group (SIGKILL), when it doesn't
    /// end on an interrupt.
    fn kill(&self) -> io::Result<()>;
    /// Blocks until the program exits.
    fn wait(&self) -> io::Result<Exit>;
}
