//! The system clipboard, behind one seam: tests get a per-thread fake that
//! never touches the user's clipboard and works where there is none.

use std::path::PathBuf;

/// Puts `image` on the clipboard. Encoding it (a PNG, and on Windows a
/// bitmap too) takes a while for a large image, during which Windows keeps
/// the clipboard emptied and locked: done by one worker, the latest copy
/// winning; a failure comes back through [`failure`].
pub(super) fn set_image(image: image::RgbaImage, ctx: &eframe::egui::Context) {
    #[cfg(test)]
    {
        let _ = ctx;
        FAKE.with_borrow_mut(|f| f.image = Some(image));
    }
    #[cfg(not(test))]
    worker::get().copy(image, ctx.clone());
}

/// Why the last copy failed, once.
pub(super) fn failure() -> Option<String> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    worker::get().failure.lock().ok()?.take()
}

/// The image on the clipboard, if any.
pub(super) fn image() -> Option<image::RgbaImage> {
    #[cfg(test)]
    {
        FAKE.with_borrow(|f| f.image.clone())
    }
    #[cfg(not(test))]
    {
        // A copy on its way lands first.
        worker::get().wait();
        let data = arboard::Clipboard::new().and_then(|mut c| c.get_image()).ok()?;
        image::RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
    }
}

#[cfg(not(test))]
mod worker {
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};

    pub(super) struct Worker {
        send: Mutex<Sender<(image::RgbaImage, eframe::egui::Context)>>,
        /// Copies not yet on the clipboard.
        pending: Arc<(Mutex<usize>, Condvar)>,
        pub(super) failure: Arc<Mutex<Option<String>>>,
    }

    pub(super) fn get() -> &'static Worker {
        static WORKER: OnceLock<Worker> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (send, receive) = channel();
            let pending = Arc::new((Mutex::new(0), Condvar::new()));
            let failure = Arc::new(Mutex::new(None));
            let (p, f) = (Arc::clone(&pending), Arc::clone(&failure));
            std::thread::Builder::new()
                .name("clipboard".into())
                .spawn(move || run(&receive, &p, &f))
                .expect("clipboard thread");
            Worker {
                send: Mutex::new(send),
                pending,
                failure,
            }
        })
    }

    fn run(
        receive: &Receiver<(image::RgbaImage, eframe::egui::Context)>,
        pending: &(Mutex<usize>, Condvar),
        failure: &Mutex<Option<String>>,
    ) {
        while let Ok(mut next) = receive.recv() {
            // Only the latest of copies made in a row.
            let mut taken = 1;
            while let Ok(later) = receive.try_recv() {
                next = later;
                taken += 1;
            }
            let (image, ctx) = next;
            let data = arboard::ImageData {
                width: image.width() as usize,
                height: image.height() as usize,
                bytes: std::borrow::Cow::Owned(image.into_raw()),
            };
            let result = arboard::Clipboard::new().and_then(|mut c| c.set_image(data));
            if let Err(e) = result
                && let Ok(mut f) = failure.lock()
            {
                *f = Some(e.to_string());
            }
            if let Ok(mut n) = pending.0.lock() {
                *n -= taken;
            }
            pending.1.notify_all();
            ctx.request_repaint();
        }
    }

    impl Worker {
        pub(super) fn copy(&self, image: image::RgbaImage, ctx: eframe::egui::Context) {
            if let Ok(mut n) = self.pending.0.lock() {
                *n += 1;
            }
            if let Ok(send) = self.send.lock() {
                let _ = send.send((image, ctx));
            }
        }

        /// Until the copies made are on the clipboard (a few seconds at
        /// most: a clipboard held by another program doesn't hang Atelier).
        pub(super) fn wait(&self) {
            let (lock, done) = &*self.pending;
            if let Ok(n) = lock.lock() {
                let _ = done.wait_timeout_while(n, std::time::Duration::from_secs(5), |n| *n > 0);
            }
        }
    }
}

/// Files copied in the file manager.
pub(super) fn files() -> Vec<PathBuf> {
    #[cfg(test)]
    {
        FAKE.with_borrow(|f| f.files.clone())
    }
    #[cfg(not(test))]
    {
        worker::get().wait();
        arboard::Clipboard::new()
            .and_then(|mut c| c.get().file_list())
            .unwrap_or_default()
    }
}

/// The clipboard in tests: one per test thread.
#[cfg(test)]
#[derive(Default)]
pub(super) struct Fake {
    pub(super) image: Option<image::RgbaImage>,
    pub(super) files: Vec<PathBuf>,
}

#[cfg(test)]
thread_local! {
    pub(super) static FAKE: std::cell::RefCell<Fake> = Default::default();
}
