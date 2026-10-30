//! Desktop services the editors ask their host for: the system clipboard (including HTML), the
//! printer spooler, and audio output for media on slides. These follow what each engine's own
//! desktop app does, so copy and paste, printing and sound behave the same inside Alfadoc.
//!
//! Not built for the browser, which has its own clipboard, print dialog and audio.

/// The system clipboard through `arboard`. The handle is kept alive because X11 and Wayland serve
/// clipboard data from the process that owns it.
pub mod clipboard {
    use std::cell::RefCell;
    use std::rc::Rc;

    pub type Handle = Rc<RefCell<Option<arboard::Clipboard>>>;

    pub fn handle() -> Handle {
        Rc::default()
    }

    /// Runs `read` on the clipboard, opening it first if needed. `None` when it is unavailable
    /// or empty (never an error the user must see: paste simply finds nothing).
    pub fn read<T>(handle: &Handle, read: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Option<T> {
        let mut clipboard = handle.try_borrow_mut().ok()?;
        if clipboard.is_none() {
            *clipboard = arboard::Clipboard::new().ok();
        }
        read(clipboard.as_mut()?).ok()
    }

    pub fn text(handle: &Handle) -> Option<String> {
        read(handle, |c| c.get_text())
    }

    pub fn html(handle: &Handle) -> Option<String> {
        read(handle, |c| c.get().html())
    }

    pub fn set_html(handle: &Handle, html: &str, text: &str) -> Result<(), String> {
        let mut clipboard = handle.try_borrow_mut().map_err(|e| e.to_string())?;
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        clipboard.as_mut().ok_or("the clipboard is not available")?.set_html(html, Some(text)).map_err(|e| e.to_string())
    }
}

/// Printer discovery and job submission on worker threads, so the spooler never blocks the window.
pub mod print {
    use std::sync::Mutex;
    use std::thread::JoinHandle;

    // Workers stay reachable until the window closes: dropping a handle would let the process end
    // before the spooler has the job.
    static JOBS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

    fn keep(worker: JoinHandle<()>) {
        let mut jobs = JOBS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        jobs.retain(|job| !job.is_finished());
        jobs.push(worker);
    }

    /// Waits for print jobs still being handed to the spooler.
    pub fn finish_pending_jobs() {
        let workers = std::mem::take(&mut *JOBS.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for worker in workers {
            if worker.join().is_err() {
                log::error!("a print worker stopped unexpectedly");
            }
        }
    }

    /// Dropped when the window closes: lets pending print jobs finish first.
    #[derive(Default)]
    pub struct ExitGuard;

    impl Drop for ExitGuard {
        fn drop(&mut self) {
            finish_pending_jobs();
        }
    }

    pub mod grid {
        use gridcraft_ui_egui::printing::{Discover, Duplex, Printer, Settings, Submit};

        pub fn discover() -> Discover {
            Box::new(|| {
                let (sender, receiver) = std::sync::mpsc::channel();
                let _ = std::thread::Builder::new().name("print-discovery".into()).spawn(move || {
                    let printers = craft_print::printers().into_iter().map(|p| Printer { name: p.name, label: p.description, default: p.default }).collect();
                    let _ = sender.send(printers);
                });
                receiver
            })
        }

        pub fn submit() -> Submit {
            Box::new(|bytes, settings: Settings| {
                let (sender, receiver) = std::sync::mpsc::channel();
                let worker = std::thread::Builder::new().name("print-submit".into()).spawn(move || {
                    let job = craft_print::Job {
                        printer: Some(settings.printer),
                        copies: settings.copies.clamp(1, 999),
                        duplex: settings.duplex.map(|value| match value {
                            Duplex::LongEdge => craft_print::Duplex::LongEdge,
                            Duplex::ShortEdge => craft_print::Duplex::ShortEdge,
                            Duplex::Off => craft_print::Duplex::Off,
                        }),
                        grayscale: settings.grayscale,
                        scaling: craft_print::Scaling::ShrinkToFit,
                        title: "Alfadoc spreadsheet".into(),
                        ..Default::default()
                    };
                    let _ = sender.send(craft_print::submit(&bytes, &job).map_err(|error| error.to_string()));
                });
                if let Ok(worker) = worker {
                    super::keep(worker);
                }
                receiver
            })
        }
    }

    pub mod word {
        use wordcraft_ui_egui::printing::{Discover, Duplex, Printer, Settings, Submit};

        pub fn discover() -> Discover {
            Box::new(|| {
                let (sender, receiver) = std::sync::mpsc::channel();
                let _ = std::thread::Builder::new().name("print-discovery".into()).spawn(move || {
                    let printers = craft_print::printers().into_iter().map(|p| Printer { name: p.name, label: p.description, default: p.default }).collect();
                    let _ = sender.send(printers);
                });
                receiver
            })
        }

        pub fn submit() -> Submit {
            Box::new(|bytes, settings: Settings, collate, options| {
                let (sender, receiver) = std::sync::mpsc::channel();
                let worker = std::thread::Builder::new().name("print-submit".into()).spawn(move || {
                    let job = craft_print::Job {
                        printer: Some(settings.printer),
                        copies: settings.copies.clamp(1, 999),
                        collate,
                        duplex: settings.duplex.map(|value| match value {
                            Duplex::LongEdge => craft_print::Duplex::LongEdge,
                            Duplex::ShortEdge => craft_print::Duplex::ShortEdge,
                            Duplex::Off => craft_print::Duplex::Off,
                        }),
                        grayscale: settings.grayscale,
                        scaling: craft_print::Scaling::ShrinkToFit,
                        title: "Alfadoc document".into(),
                        options,
                    };
                    let _ = sender.send(craft_print::submit(&bytes, &job).map_err(|error| error.to_string()));
                });
                if let Ok(worker) = worker {
                    super::keep(worker);
                }
                receiver
            })
        }
    }
}

/// cpal audio output for media on slides: the device pulls mixed samples from the media player,
/// which makes it the playback clock the video frames follow.
pub mod audio {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use deckcraft_media::AudioOut;

    /// The first stream error since it was last reported. cpal calls the error callback on its
    /// realtime audio thread, which must not log; the UI thread logs it when the stream stops.
    type ErrorSlot = Arc<Mutex<Option<String>>>;

    fn keep_stream_error(slot: &ErrorSlot, e: &dyn std::fmt::Display) {
        if let Ok(mut kept) = slot.try_lock()
            && kept.is_none()
        {
            *kept = Some(e.to_string());
        }
    }

    fn take_stream_error(slot: &ErrorSlot) -> Option<String> {
        slot.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    #[derive(Default)]
    pub struct CpalOut {
        stream: Option<cpal::Stream>,
        error: ErrorSlot,
        /// Set by the realtime error callback when the device went away: the stream is dead.
        dead: Arc<AtomicBool>,
    }

    impl CpalOut {
        fn report(&self) {
            if let Some(e) = take_stream_error(&self.error) {
                log::warn!("audio stream error: {e}");
            }
        }
    }

    impl Drop for CpalOut {
        fn drop(&mut self) {
            self.stream = None;
            self.report();
        }
    }

    impl AudioOut for CpalOut {
        fn start(&mut self, mut fill: Box<dyn FnMut(&mut [f32], usize, u32) + Send>) -> Result<(), String> {
            self.stop();
            let dev = cpal::default_host().default_output_device().ok_or("no audio output device")?;
            let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
            let channels = cfg.channels() as usize;
            let rate = cfg.sample_rate().0;
            let config: cpal::StreamConfig = cfg.clone().into();
            let slot = self.error.clone();
            let dead = Arc::new(AtomicBool::new(false));
            self.dead = dead.clone();
            let err = move |e: cpal::StreamError| {
                if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                    dead.store(true, Ordering::Relaxed);
                }
                keep_stream_error(&slot, &e);
            };
            let stream = match cfg.sample_format() {
                cpal::SampleFormat::F32 => dev.build_output_stream(&config, move |buf: &mut [f32], _| fill(buf, channels, rate), err, None),
                cpal::SampleFormat::I16 => {
                    let mut tmp = Vec::new();
                    dev.build_output_stream(
                        &config,
                        move |buf: &mut [i16], _| {
                            tmp.resize(buf.len(), 0.0);
                            fill(&mut tmp, channels, rate);
                            for (o, s) in buf.iter_mut().zip(&tmp) {
                                *o = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                            }
                        },
                        err,
                        None,
                    )
                }
                other => return Err(format!("unsupported sample format {other:?}")),
            }
            .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            self.stream = Some(stream);
            Ok(())
        }

        fn stop(&mut self) {
            self.stream = None;
            self.report();
        }

        fn failure(&mut self) -> Option<String> {
            if !self.dead.swap(false, Ordering::Relaxed) {
                return None;
            }
            Some(take_stream_error(&self.error).unwrap_or_else(|| "the audio device is no longer available".to_string()))
        }
    }
}
