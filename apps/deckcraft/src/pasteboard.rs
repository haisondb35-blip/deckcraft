//! System pasteboard: HTML rich text, and a picture (TIFF on macOS, via arboard).
//!
//! Pages keeps bold, italic and underline when the pasteboard carries HTML plus the same plain
//! words. A shape with no text is copied as an image by the UI; this module only reads a picture
//! back (Keynote's TIFF) as PNG bytes.

/// HTML currently on the pasteboard, when there is some.
pub fn html() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let html = cb.get().html().ok()?;
    let html = html.trim();
    if html.is_empty() { None } else { Some(html.to_string()) }
}

/// Write `html` and the plain alternative `text`. `false` when the pasteboard refuses it.
pub fn set_html(html: &str, text: &str) -> bool {
    let Ok(mut cb) = arboard::Clipboard::new() else { return false };
    cb.set_html(html, Some(text)).is_ok()
}

/// Copied slides or objects, the way PowerPoint puts its own on the Windows clipboard: one open
/// of the clipboard holds the plain `text`, the `html` when there is some, the picture as PNG and
/// as a DIB (other apps: Paint, PhotoCraft, a mail) and DeckCraft's own `objects` bytes under the
/// registered format `DeckCraft Objects`, which every DeckCraft window pastes. A copy anywhere
/// else empties the clipboard, so old objects never come back.
#[cfg(windows)]
pub fn set_objects(text: &str, html: Option<&str>, png: Option<&[u8]>, objects: &[u8]) -> bool {
    use clipboard_win::{formats, options::NoClear, raw};
    let Ok(_open) = clipboard_win::Clipboard::new_attempts(10) else { return false };
    if raw::empty().is_err() || raw::set_string_with(text, NoClear).is_err() {
        return false;
    }
    if let Some(html) = html
        && let Some(f) = formats::Html::new()
    {
        let _ = raw::set_html_with(f.code(), html, NoClear);
    }
    if let Some(png) = png {
        if let Some(f) = raw::register_format("PNG") {
            let _ = raw::set_without_clear(f.get(), png);
        }
        if let Some(dib) = dib(png) {
            let _ = raw::set_without_clear(formats::CF_DIB, &dib);
        }
    }
    raw::register_format(deckcraft_engine::clipboard_data::FORMAT).is_some_and(|f| raw::set_without_clear(f.get(), objects).is_ok())
}

/// DeckCraft's own bytes on the Windows clipboard, when slides or objects were copied last.
#[cfg(windows)]
pub fn objects() -> Option<Vec<u8>> {
    use clipboard_win::raw;
    let f = raw::register_format(deckcraft_engine::clipboard_data::FORMAT)?;
    if !raw::is_format_avail(f.get()) {
        return None;
    }
    let _open = clipboard_win::Clipboard::new_attempts(10).ok()?;
    let mut out = Vec::new();
    raw::get_vec(f.get(), &mut out).ok()?;
    Some(out)
}

/// `png` as a packed DIB (BITMAPINFOHEADER and bottom-up 24-bit rows), over white: what apps
/// that don't read PNG from the clipboard take.
#[cfg(any(windows, test))]
fn dib(png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let row = (usize::try_from(w).ok()? * 3).div_ceil(4) * 4;
    let size = row.checked_mul(usize::try_from(h).ok()?)?;
    let mut out = Vec::with_capacity(40 + size);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&i32::try_from(w).ok()?.to_le_bytes());
    out.extend_from_slice(&i32::try_from(h).ok()?.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&u32::try_from(size).ok()?.to_le_bytes());
    out.extend_from_slice(&[0; 16]); // resolution and palette: none
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            let over = |c: u8| ((u16::from(c) * u16::from(a) + 255 * (255 - u16::from(a))) / 255) as u8;
            out.extend_from_slice(&[over(b), over(g), over(r)]);
        }
        out.resize(start + row, 0);
    }
    Some(out)
}

/// The pasteboard picture as PNG bytes (TIFF on macOS is decoded by arboard).
pub fn image_png() -> Option<Vec<u8>> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    let width = u32::try_from(img.width).ok()?;
    let height = u32::try_from(img.height).ok()?;
    let rgba = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?;
    let mut out = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
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

    #[test]
    fn a_dib_is_bottom_up_over_white() {
        // 1 × 2: a transparent pixel on top of a red one.
        let img = image::RgbaImage::from_raw(1, 2, vec![0, 0, 0, 0, 255, 0, 0, 255]).expect("image");
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png");
        let dib = super::dib(&png).expect("dib");
        assert_eq!(dib.len(), 40 + 2 * 4);
        assert_eq!(&dib[40..43], &[0, 0, 255], "the bottom row (red, as BGR) comes first");
        assert_eq!(&dib[44..47], &[255, 255, 255], "transparent becomes white");
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
