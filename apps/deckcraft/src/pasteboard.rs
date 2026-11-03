//! System pasteboard: HTML rich text, and a picture (TIFF on macOS, via arboard).
//!
//!
//! Pages keeps bold, italic and underline when the pasteboard carries HTML plus the same plain
//! words. A shape with no text is copied as an image by the UI; this module only reads a picture
//! back (Keynote's TIFF) as PNG bytes.

//! On Windows through the shared craft-clipboard crate: what every Craft app and other program
//! reads (text, HTML, PNG and bitmaps, files copied in Explorer) plus DeckCraft's own format.

/// HTML currently on the pasteboard, when there is some.
pub fn html() -> Option<String> {
    #[cfg(windows)]
    let html = craft_clipboard::html()?.fragment;
    #[cfg(not(windows))]
    let html = arboard::Clipboard::new().ok()?.get().html().ok()?;
    let html = html.trim();
    if html.is_empty() { None } else { Some(html.to_string()) }
}

/// Write `html` and the plain alternative `text`. `false` when the pasteboard refuses it.
pub fn set_html(html: &str, text: &str) -> bool {
    #[cfg(windows)]
    return craft_clipboard::write(&craft_clipboard::Copy { text: Some(text.into()), html: Some(html.into()), ..Default::default() }).is_ok();
    #[cfg(not(windows))]
    {
        let Ok(mut cb) = arboard::Clipboard::new() else { return false };
        cb.set_html(html, Some(text)).is_ok()
    }
}

/// Copied slides or objects, the way PowerPoint puts its own on the Windows clipboard: one open
/// of the clipboard holds the plain `text`, the `html` when there is some, the picture as PNG and
/// as bitmaps (other apps: Paint, PhotoCraft, a mail) and DeckCraft's own `objects` bytes under the
/// registered format `DeckCraft Objects`, which every DeckCraft window pastes. A copy anywhere
/// else empties the clipboard, so old objects never come back.
#[cfg(windows)]
pub fn set_objects(text: &str, html: Option<&str>, png: Option<&[u8]>, objects: &[u8]) -> bool {
    craft_clipboard::write(&craft_clipboard::Copy {
        text: Some(text.into()),
        html: html.map(str::to_string),
        png: png.map(<[u8]>::to_vec),
        own: vec![(deckcraft_engine::clipboard_data::FORMAT.to_string(), objects.to_vec())],
        ..Default::default()
    })
    .is_ok()
}

/// DeckCraft's own bytes on the Windows clipboard, when slides or objects were copied last.
#[cfg(windows)]
pub fn objects() -> Option<Vec<u8>> {
    craft_clipboard::own(deckcraft_engine::clipboard_data::FORMAT)
}

/// `img` as PNG bytes.
fn png_of(img: &image::RgbaImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// The pasteboard picture as PNG bytes. Windows: a bitmap, else the first picture file copied in
/// Explorer, else the one picture of copied HTML (a web page's), converted to PNG; SVG isn't drawn
/// here. Elsewhere arboard's bitmap (Keynote's TIFF decoded).
pub fn image_png() -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        let p = craft_clipboard::picture()?;
        if p.mime == craft_clipboard::media::PNG {
            return Some(p.data);
        }
        if p.mime == craft_clipboard::media::SVG {
            return None;
        }
        png_of(&image::load_from_memory(&p.data).ok()?.to_rgba8())
    }
    #[cfg(not(windows))]
    {
        let mut cb = arboard::Clipboard::new().ok()?;
        let img = cb.get_image().ok()?;
        let width = u32::try_from(img.width).ok()?;
        let height = u32::try_from(img.height).ok()?;
        png_of(&image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?)
    }
}
#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    /// Puts HTML and a 1×1 picture on the macOS pasteboard and reads them back.
    /// Restores whatever plain text was there. Other platforms return immediately.
    #[test]
    fn html_and_a_picture_round_trip_on_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let previous = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok());
        let _restore = Restore(previous);
        assert!(super::set_html("<b>DeckCraft</b>", "DeckCraft"), "set html");
        let html = super::html().unwrap_or_default();
        assert!(html.contains("<b>") || html.contains("<B>"), "{html}");
        let text = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok()).unwrap_or_default();
        assert_eq!(text, "DeckCraft");
        let mut cb = arboard::Clipboard::new().expect("pasteboard");
        let img = arboard::ImageData { width: 1, height: 1, bytes: Cow::Owned(vec![255, 0, 0, 255]) };
        cb.set_image(img).expect("set image");
        drop(cb);
        let png = super::image_png().expect("png");
        let decoded = image::load_from_memory(&png).expect("decode").to_rgba8();
        let p = decoded.get_pixel(0, 0);
        assert!(p[0] > 200 && p[1] < 40 && p[3] > 200, "{p:?}");
    }

    /// Puts text, HTML, a picture and DeckCraft's own bytes on the Windows clipboard in one go
    /// and reads them back; another copy (plain text) removes the objects. Restores the text.
    #[cfg(windows)]
    #[test]
    fn objects_ride_next_to_text_and_a_picture_on_windows() {
        let previous = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok());
        let _restore = Restore(previous);
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 128, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png");
        assert!(super::set_objects("Title", Some("<b>Title</b>"), Some(&png), b"DECKCLP1 bytes"), "set objects");
        assert_eq!(super::objects().as_deref(), Some(&b"DECKCLP1 bytes"[..]));
        let mut cb = arboard::Clipboard::new().expect("clipboard");
        assert_eq!(cb.get_text().ok().as_deref(), Some("Title"));
        assert!(cb.get_image().is_ok(), "other apps get the picture");
        cb.set_text("from another app").expect("set text");
        drop(cb);
        assert_eq!(super::objects(), None, "a copy elsewhere replaces the objects");
    }

    /// Puts the previous plain text back, including when the test fails halfway.
    struct Restore(Option<String>);

    impl Drop for Restore {
        fn drop(&mut self) {
            let Ok(mut cb) = arboard::Clipboard::new() else { return };
            match self.0.take() {
                Some(t) => {
                    let _ = cb.set_text(t);
                }
                None => {
                    let _ = cb.clear();
                }
            }
        }
    }
}
