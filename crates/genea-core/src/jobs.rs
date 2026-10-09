//! Background work and its way back to the main thread.
//!
//! The rule (spec #19, Threading): nothing on the main thread blocks on I/O
//! or on another process. Work runs on a background thread and produces an
//! [`Apply`], a closure that changes core state. Applies queue up in the
//! workbench's inbox, and the workbench runs them on the main thread in
//! `pump` or `settle`.
//!
//! Every job counts as *pending* from the moment it is spawned until its
//! Apply has run. `settle` returns once nothing is pending, which is what
//! "the core is quiescent" means. Long-lived sources (a watcher, an LSP
//! reader) must hold a [`Busy`] token while they are processing something, so
//! that settle waits for them too.

use std::{
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};

use crate::workbench::Core;

/// A change to core state, computed in the background, run on the main thread.
pub(crate) type Apply = Box<dyn FnOnce(&mut Core) + Send + 'static>;

/// Called from any thread when there is new work for `pump`.
pub(crate) type Notifier = Arc<dyn Fn() + Send + Sync + 'static>;

/// The sending side: cheap to clone, safe to use from any thread.
#[derive(Clone)]
pub(crate) struct Jobs {
    tx: Sender<Apply>,
    pending: Arc<AtomicUsize>,
    notifier: Arc<RwLock<Option<Notifier>>>,
}

/// The receiving side, owned by the workbench.
pub(crate) struct Inbox {
    rx: Receiver<Apply>,
    pending: Arc<AtomicUsize>,
}

pub(crate) fn channel() -> (Jobs, Inbox) {
    let (tx, rx) = mpsc::channel();
    let pending = Arc::new(AtomicUsize::new(0));
    (
        Jobs { tx, pending: pending.clone(), notifier: Arc::default() },
        Inbox { rx, pending },
    )
}

impl Jobs {
    /// Runs `work` on a background thread and queues the Apply it returns.
    ///
    /// A panic in `work` is re-raised on the main thread when its Apply
    /// runs, so it fails the test (or the app) instead of hanging `settle`.
    pub(crate) fn spawn<F>(&self, name: &str, work: F)
    where
        F: FnOnce() -> Apply + Send + 'static,
    {
        let busy = self.busy();
        std::thread::Builder::new()
            .name(format!("genea: {name}"))
            .spawn(move || {
                let apply = match panic::catch_unwind(AssertUnwindSafe(work)) {
                    Ok(apply) => apply,
                    Err(payload) => Box::new(move |_: &mut Core| panic::resume_unwind(payload)),
                };
                busy.finish(apply);
            })
            .expect("spawn a background thread");
    }

    /// Marks work in progress on a long-lived thread. Finish the token with
    /// the work's Apply; dropping it unfinished just ends the work.
    pub(crate) fn busy(&self) -> Busy {
        self.pending.fetch_add(1, Ordering::SeqCst);
        Busy { jobs: Some(self.clone()) }
    }

    pub(crate) fn set_notifier(&self, notifier: Option<Notifier>) {
        *self.notifier.write().unwrap() = notifier;
    }

    fn post(&self, apply: Apply) {
        // The receiver lives as long as the workbench; after that, results
        // have nowhere to go and are dropped.
        let _ = self.tx.send(apply);
        let notifier = self.notifier.read().unwrap().clone();
        if let Some(notify) = notifier {
            notify();
        }
    }
}

/// Work in progress. See [`Jobs::busy`].
pub(crate) struct Busy {
    jobs: Option<Jobs>,
}

impl Busy {
    pub(crate) fn finish(mut self, apply: Apply) {
        if let Some(jobs) = self.jobs.take() {
            jobs.post(apply);
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(jobs) = self.jobs.take() {
            jobs.post(Box::new(|_| {}));
        }
    }
}

impl Inbox {
    pub(crate) fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    pub(crate) fn try_next(&self) -> Option<Apply> {
        self.rx.try_recv().ok()
    }

    pub(crate) fn next_timeout(&self, timeout: std::time::Duration) -> Option<Apply> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Call after an Apply from this inbox has run.
    pub(crate) fn done(&self) {
        self.pending.fetch_sub(1, Ordering::SeqCst);
    }
}
