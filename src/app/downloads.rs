//! Model downloads in the background, one per model, shared by the AI
//! features and the AI Models window.

use std::sync::{Arc, mpsc};

use crate::ai::models::{DownloadError, Model, Progress};
use crate::i18n::t;

struct Job {
    model: Model,
    progress: Arc<Progress>,
    done: mpsc::Receiver<Result<(), DownloadError>>,
}

#[derive(Default)]
pub(super) struct Downloads {
    jobs: Vec<Job>,
    /// Cancelled, not reported yet.
    cancelled: Vec<Model>,
}

impl Downloads {
    /// Starts downloading `model` (nothing if it is already downloading).
    pub(super) fn start(&mut self, model: Model) {
        if self.is_downloading(model) {
            return;
        }
        let progress = Arc::new(Progress::default());
        let shared = Arc::clone(&progress);
        let (send, done) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(model.download(&shared));
        });
        self.jobs.push(Job {
            model,
            progress,
            done,
        });
    }

    pub(super) fn is_downloading(&self, model: Model) -> bool {
        self.jobs.iter().any(|j| j.model == model)
    }

    /// Downloaded fraction (0..1) of `model`, while it downloads.
    pub(super) fn progress(&self, model: Model) -> Option<f32> {
        self.jobs
            .iter()
            .find(|j| j.model == model)
            .map(|j| j.progress.fraction())
    }

    /// Stops the download of `model` at once: its thread ends on its own
    /// (its partial file is removed), and a new download can start.
    pub(super) fn cancel(&mut self, model: Model) {
        self.jobs.retain(|job| {
            if job.model != model {
                return true;
            }
            job.progress
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            false
        });
        self.cancelled.push(model);
    }

    /// Downloads that ended since the last call, with their outcome.
    pub(super) fn finished(&mut self) -> Vec<(Model, Result<(), DownloadError>)> {
        let mut ended: Vec<_> = self
            .cancelled
            .drain(..)
            .map(|model| (model, Err(DownloadError::Cancelled)))
            .collect();
        self.jobs.retain(|job| match job.done.try_recv() {
            Ok(result) => {
                ended.push((job.model, result));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let error = DownloadError::Failed(t!("ai-error-download-interrupted"));
                ended.push((job.model, Err(error)));
                false
            }
        });
        ended
    }

    pub(super) fn any(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// A download stuck at `fraction`, for captures.
    #[cfg(test)]
    pub(super) fn simulate(&mut self, model: Model, fraction: f32) {
        use std::sync::atomic::Ordering::Relaxed;
        let progress = Arc::new(Progress::default());
        progress.total.store(1000, Relaxed);
        progress
            .downloaded
            .store((fraction * 1000.0) as u64, Relaxed);
        let (send, done) = mpsc::channel();
        std::mem::forget(send);
        self.jobs.push(Job {
            model,
            progress,
            done,
        });
    }
}
