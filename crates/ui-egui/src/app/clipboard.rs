use deckcraft_engine::clipboard_data;

use super::{SlideApp, Value, json};

/// Text put on the system clipboard when the copied objects (a rectangle, a picture, slides…)
/// carry no text. egui-winit drops Ctrl+V when the system clipboard is empty, so without it the
/// shortcut never reached the session and only the context menu could paste objects (#182). An
/// invisible word joiner, so a paste into another app adds nothing visible.
pub(crate) const OBJECTS_MARKER: &str = "\u{2060}";

/// What a copy/cut leaves on the system clipboard for the session's plain-text rendering `t`.
pub(crate) fn system_clipboard_text(t: &str) -> String {
    if t.is_empty() { OBJECTS_MARKER.to_string() } else { t.to_string() }
}

/// Whether pasted `text` (and the clipboard's `html`) only name a picture that is on the clipboard
/// too, as a browser's Copy Image leaves them: the picture's address, perhaps followed by its alt
/// text, or HTML that is a lone `<img>`. Then Ctrl+V pastes the picture, as PowerPoint does.
/// Text with NULs is UTF-16 read as UTF-8 (Firefox's `text/x-moz-url`), never words to keep.
pub(crate) fn names_a_picture(text: &str, html: Option<&str>) -> bool {
    if text.contains('\0') {
        return true;
    }
    if let Some(h) = html
        && h.to_ascii_lowercase().contains("<img")
        && visible_text(h).trim().is_empty()
    {
        return true;
    }
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or("").trim();
    let link = ["http://", "https://", "file://", "data:image/"].iter().any(|p| first.to_ascii_lowercase().starts_with(p));
    link && !first.contains(char::is_whitespace) && lines.count() <= 1
}

/// The text of an HTML fragment outside its tags (and outside comments and `<style>`/`<script>`
/// bodies, which browsers sometimes put around a copied picture).
fn visible_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        let skip_to = |end: &str| rest.find(end).map(|e| i + e + end.len()).unwrap_or(html.len());
        if rest.starts_with("<!--") {
            i = skip_to("-->");
        } else if rest.starts_with("<style") {
            i = skip_to("</style>");
        } else if rest.starts_with("<script") {
            i = skip_to("</script>");
        } else if rest.starts_with('<') {
            i = skip_to(">");
        } else {
            let end = rest.find('<').map_or(html.len(), |e| i + e);
            out.push_str(&html[i..end]);
            i = end;
        }
    }
    out.replace("&nbsp;", " ")
}

impl SlideApp {
    /// Offer the copy or cut on the system clipboard.
    ///
    /// Slides go out as plain text. Text goes out as HTML plus the same plain words, so a paste
    /// back into DeckCraft still uses the internal clipboard. A textless shape leaves the
    /// invisible [`OBJECTS_MARKER`] rather than a picture: with only an image on the clipboard
    /// egui-winit drops Ctrl+V, and the shortcut could no longer paste the shape here (#182).
    pub(super) fn publish_clipboard(&mut self, ctx: &egui::Context, cut: bool) {
        let cmd = if cut { "edit.cut" } else { "edit.copy" };
        let Ok(v) = self.session.execute(cmd, &json!({})) else { return };
        let text = v.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        let html = v.get("html").and_then(Value::as_str).unwrap_or("").to_string();
        if self.publish_objects(&text, &html) {
            return;
        }
        if self.session.clipboard.slides.is_empty() && !html.is_empty() && self.services.clipboard_set_html.as_mut().is_some_and(|f| f(&html, &text))
        {
            return;
        }
        ctx.copy_text(system_clipboard_text(&text));
    }

    /// Put copied slides or objects on the system clipboard with DeckCraft's own bytes, so any
    /// DeckCraft window pastes them as they are, and a picture of them for other apps. `false`
    /// when the copy holds neither or the host can't (then the text goes out as before).
    pub(super) fn publish_objects(&mut self, text: &str, html: &str) -> bool {
        if self.services.clipboard_set_objects.is_none() {
            return false;
        }
        let stamp = clipboard_data::new_stamp();
        let Some(bytes) = clipboard_data::encode(&self.session.clipboard, stamp) else { return false };
        let png = deckcraft_engine::cmd::edit::clipboard_png(&self.session);
        let html = (!html.is_empty() && self.session.clipboard.slides.is_empty()).then_some(html);
        let text = system_clipboard_text(text);
        let landed = self.services.clipboard_set_objects.as_mut().is_some_and(|f| f(&text, html, png.as_deref(), &bytes));
        if landed {
            self.clip_stamp = stamp;
        }
        landed
    }

    /// Before a paste: when the system clipboard holds slides or objects some DeckCraft window
    /// copied (this one or another), paste those. `true` when it does; a copy this window made
    /// keeps the session's clipboard.
    pub(super) fn adopt_clipboard_objects(&mut self) -> bool {
        let Some(read) = self.services.clipboard_objects.as_mut() else { return false };
        let Some((stamp, clip)) = read().and_then(|b| clipboard_data::decode(&b)) else { return false };
        if stamp != self.clip_stamp {
            self.clip_stamp = stamp;
            self.session.clipboard = clip;
        }
        true
    }
}
