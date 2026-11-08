//! Crash recovery in the window: the modified documents written aside every
//! few minutes, on another thread (see `document::recovery`); what a window
//! that ended without closing them left, offered at start. Also what holds
//! the window's close: a workspace whose work is nowhere else, or work
//! under way that hasn't reached its document (or its files) yet.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use super::*;
use crate::document::recovery::{Folder, Left};

/// How often the modified documents are written aside, in seconds.
const EVERY: f64 = 10.0 * 60.0;

/// What was written aside: document, state, outcome.
type Written = Vec<(u64, u64, Result<(), String>)>;

#[derive(Default)]
pub(super) struct Recovery {
    /// This window's folder (none in tests, unless one asks).
    folder: Option<Arc<Folder>>,
    /// Document → the state written aside.
    written: HashMap<u64, u64>,
    /// When the documents were last written aside (input time).
    last: Option<f64>,
    job: Option<Receiver<Written>>,
    /// Told once that recovery files can't be written.
    failed: bool,
    /// What windows that ended without closing their documents left.
    offer: Vec<(Folder, Vec<Left>)>,
    /// The window's close waits for work under way.
    close_waits: bool,
}

impl Recovery {
    /// This window's folder, and what earlier ones left behind in `root`.
    pub(super) fn start(root: &Path) -> Self {
        let offer = Folder::left_in(root);
        let folder = match Folder::start(root) {
            Ok(f) => Some(Arc::new(f)),
            Err(e) => {
                crate::diag::warn(tm!(
                    "safety14-recovery-failed",
                    error = crate::i18n::Message::of(|| crate::i18n::io_error(&e))
                ));
                None
            }
        };
        let mut recovery = Self::default();
        (recovery.folder, recovery.offer) = (folder, offer);
        recovery
    }

    /// What earlier windows left is being offered.
    pub(super) fn asking(&self) -> bool {
        !self.offer.is_empty()
    }
}

impl Drop for Recovery {
    /// The window closes as asked: its folder goes (every document was
    /// saved, or the user chose not to). Not after a panic: what it holds
    /// is offered at the next start.
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Some(job) = self.job.take() {
            let _ = job.recv();
        }
        if let Some(folder) = self.folder.take().and_then(|f| Arc::try_unwrap(f).ok()) {
            folder.remove();
        }
    }
}

impl AtelierApp {
    /// The open documents: identity, state, modified.
    fn tab_states(&self) -> Vec<(u64, u64, bool)> {
        (0..self.tab_count())
            .filter_map(|i| self.tab(i))
            .map(|(d, _)| (d.uid(), d.state_id(), d.modified))
            .collect()
    }

    /// Each frame: what was written aside lands; the copies of documents
    /// saved or closed go; every few minutes, the modified documents are
    /// written aside again. A close waiting for work under way goes on
    /// once it is done.
    pub(super) fn recovery_frame(&mut self, ctx: &egui::Context) {
        if self.recovery.close_waits && !self.work_under_way() {
            self.recovery.close_waits = false;
            self.run(Action::Quit);
        }
        let Some(folder) = self.recovery.folder.clone() else {
            return;
        };
        let r = &mut self.recovery;
        if let Some(job) = &r.job {
            match job.try_recv() {
                Err(TryRecvError::Empty) => return,
                Ok(written) => {
                    for (uid, state, outcome) in written {
                        match outcome {
                            Ok(()) => {
                                r.written.insert(uid, state);
                            }
                            Err(error) if !std::mem::replace(&mut r.failed, true) => {
                                crate::diag::warn(tm!("safety14-recovery-failed", error = error));
                            }
                            Err(_) => {}
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => {}
            }
            r.job = None;
        }
        let tabs = self.tab_states();
        let r = &mut self.recovery;
        r.written.retain(|uid, _| {
            let kept = tabs.iter().any(|&(u, _, modified)| u == *uid && modified);
            if !kept {
                folder.forget(*uid);
            }
            kept
        });
        let now = ctx.input(|i| i.time);
        let last = *r.last.get_or_insert(now);
        let due: Vec<u64> = tabs
            .iter()
            .filter(|&&(uid, state, modified)| modified && r.written.get(&uid) != Some(&state))
            .map(|&(uid, ..)| uid)
            .collect();
        if due.is_empty() {
            r.last = Some(now);
            return;
        }
        let wait = last + EVERY - now;
        // Not in the middle of a gesture, nor while work lands.
        let busy = ctx.input(|i| i.pointer.any_down()) || self.modal_open();
        if wait > 0.0 || busy {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(1.0)));
            return;
        }
        self.write_recovery(&due);
    }

    /// Writes the documents `due` aside, on another thread.
    fn write_recovery(&mut self, due: &[u64]) {
        let Some(folder) = self.recovery.folder.clone() else {
            return;
        };
        let copies: Vec<_> = (0..self.tab_count())
            .filter_map(|i| self.tab(i))
            .map(|(d, _)| d)
            .filter(|d| due.contains(&d.uid()))
            .map(|d| (d.uid(), d.state_id(), d.recovery_later()))
            .collect();
        let (send, receive) = channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Recovery");
            let written = copies
                .into_iter()
                .map(|(uid, state, later)| {
                    let outcome = folder.write(uid, later).map_err(|e| e.to_string());
                    (uid, state, outcome)
                })
                .collect();
            let _ = send.send(written);
            ctx.request_repaint();
        });
        self.recovery.job = Some(receive);
        self.recovery.last = Some(self.ctx.input(|i| i.time));
    }

    /// At start: "Recover documents?" for what earlier windows left.
    pub(super) fn recovery_window(&mut self, ctx: &egui::Context) {
        if self.recovery.offer.is_empty() {
            return;
        }
        let names: Vec<String> = self
            .recovery
            .offer
            .iter()
            .flat_map(|(_, left)| left.iter().map(|l| l.about.name.clone()))
            .collect();
        let center = ctx.content_rect().center();
        let out = dialogs::floating_dialog(
            ctx,
            dialogs::DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 420.0,
                ok: t!("safety14-recovery-open"),
                other: Some(t!("safety14-recovery-discard")),
                first_field: false,
                ..dialogs::DialogSpec::new("recovery", t!("safety14-recovery-title"), center)
            },
            |ui| {
                ui.label(t!("safety14-recovery-text", count = names.len()));
                dialogs::gap_between_blocks(ui);
                for name in &names {
                    ui.label(format!("• {name}"));
                }
                dialogs::gap_between_blocks(ui);
                dialogs::help(ui, &t!("safety14-recovery-hint"));
            },
        );
        if !out.ok && !out.other && !out.cancel {
            return;
        }
        let offer = std::mem::take(&mut self.recovery.offer);
        if out.ok {
            for (folder, left) in offer {
                self.recover(folder, &left);
            }
        } else if out.other {
            for (folder, _) in offer {
                folder.remove();
            }
        }
        // Cancelled: offered again at the next start (unlocked).
    }

    /// Opens the documents a folder left, as modified documents; they
    /// become this window's recovery files. The folder goes once all are
    /// back.
    fn recover(&mut self, folder: Folder, left: &[Left]) {
        let mut all = true;
        for l in left {
            match crate::document::Document::recovered(l) {
                Ok(doc) => {
                    let (uid, state) = (doc.uid(), doc.state_id());
                    if let Some(own) = &self.recovery.folder {
                        own.take(l, uid);
                        self.recovery.written.insert(uid, state);
                    }
                    self.add_tab(doc);
                }
                Err(e) => {
                    all = false;
                    self.report_error(tm!(
                        "status-open-failed",
                        path = l.about.name.clone(),
                        error = crate::i18n::Message::of(|| e.to_string())
                    ));
                }
            }
        }
        if all {
            folder.remove();
        }
        self.status.set(t!("safety14-status-recovered", count = left.len()));
    }

    /// Work that lands in a document, or writes files, still under way.
    pub(super) fn work_under_way(&self) -> bool {
        self.liquify.applying()
            || self.camera_raw.working()
            || self.retouch.busy()
            || self.filters.working()
            || self.artboards.exporting()
    }

    /// The window's close asked while a workspace holds work found nowhere
    /// else (Liquify, Camera Raw): refused, saying why.
    pub(super) fn close_refused(&mut self) -> bool {
        let why = if self.liquify.open() {
            tm!("safety14-close-liquify-open")
        } else if self.camera_raw.open() {
            tm!("safety14-close-camera-raw-open")
        } else {
            return false;
        };
        self.report_warning(why);
        true
    }

    /// The window's close asked while work under way hasn't reached its
    /// document or its files: it waits for it, then goes on as Quit does
    /// (asking about unsaved changes).
    pub(super) fn close_waits(&mut self) -> bool {
        if !self.work_under_way() {
            return false;
        }
        if !std::mem::replace(&mut self.recovery.close_waits, true) {
            self.status.set(t!("safety14-status-close-waits"));
        }
        true
    }

    /// Recovery files written into `root`, for tests.
    #[cfg(test)]
    pub(super) fn recover_into(&mut self, root: &Path) {
        self.recovery = Recovery::start(root);
    }

    /// As after a crash: this window's folder is left as it is.
    #[cfg(test)]
    pub(super) fn crash_recovery(&mut self) {
        self.recovery.folder = None;
    }

    /// The modified documents written aside now, waited for (tests).
    #[cfg(test)]
    pub(super) fn write_recovery_now(&mut self) {
        let due: Vec<u64> = self
            .tab_states()
            .into_iter()
            .filter(|&(_, _, modified)| modified)
            .map(|(uid, ..)| uid)
            .collect();
        self.write_recovery(&due);
        let written = self.recovery.job.as_ref().and_then(|j| j.recv().ok());
        self.recovery.job = None;
        for (uid, state, outcome) in written.into_iter().flatten() {
            outcome.unwrap();
            self.recovery.written.insert(uid, state);
        }
    }
}
