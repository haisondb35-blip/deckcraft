use super::{SlideApp, json};
use crate::control;

impl SlideApp {
    /// Drain pending control requests, executing ordinary requests and admitting screenshots for
    /// later saving and reply.
    pub(super) fn drain_control(&mut self, ctx: &egui::Context) {
        // Open from the start and listed for the other Craft apps (once).
        self.assistant.share(ctx);
        // The Claude pane's own channel, then the `--control` one.
        if let Some(rx) = self.assistant.control_rx.take() {
            self.drain_from(&rx, ctx);
            self.assistant.control_rx = Some(rx);
        }
        let Some(rx) = self.control_rx.take() else { return };
        self.drain_from(&rx, ctx);
        self.control_rx = Some(rx);
    }

    fn drain_from(&mut self, rx: &std::sync::mpsc::Receiver<crate::ControlRequest>, ctx: &egui::Context) {
        while let Ok(req) = rx.try_recv() {
            // A request whose sender gave up waiting (reply timeout) must not run late (#719).
            if !req.take() {
                continue;
            }
            let reply = req.reply.clone();
            let revision = self.session.active().map(|d| d.revision);
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { path } => {
                    let doc = self.session.active().map(|d| d.uid);
                    if let Err(e) = self.capture.admit(path, reply.clone(), doc, web_time::Instant::now()) {
                        let _ = reply.send(json!({"ok": false, "error": e}));
                    }
                }
            }
            if self.session.active().map(|d| d.revision) != revision {
                self.note_application_edit(ctx);
            }
        }
    }

    /// Save completed screenshots on the app thread and send exactly one reply each.
    pub(super) fn drain_screenshots(&mut self) {
        let doc = self.session.active().map(|d| d.uid);
        let completed = self.capture.drain(web_time::Instant::now(), doc);
        for c in completed {
            let v = control::save_screenshot(self, c.image.as_ref(), c.path.as_deref());
            // Keep the aggregate charged through RGBA/PNG/base64 encoding and the write.
            self.capture.release(c.reserved);
            let _ = c.reply.send(v);
        }
    }
}
