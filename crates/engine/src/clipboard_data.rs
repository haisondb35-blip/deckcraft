//! Copied slides and objects as bytes for the system clipboard, so every DeckCraft window (each
//! its own process) pastes them as slides and objects, as PowerPoint does with its own clipboard
//! format. The host puts the bytes on the system clipboard next to the text, HTML or picture other
//! apps read (Windows: the registered format [`FORMAT`]); copying anything anywhere else replaces
//! them, so a paste never brings back an older copy.
//!
//! Layout: [`MAGIC`], the JSON length (u64 little-endian), the JSON (the clipboard without media
//! bytes, plus the copy's stamp), then each media item's length (u64 LE) and bytes in order.

use std::sync::Arc;

use deckcraft_model::{MediaItem, Shape, Slide, TextBody};
use serde::{Deserialize, Serialize};

use crate::Clipboard;

/// The clipboard format's registered name.
pub const FORMAT: &str = "DeckCraft Objects";
const MAGIC: &[u8; 8] = b"DECKCLP1";

#[derive(Serialize, Deserialize)]
struct Data {
    stamp: u64,
    plain: String,
    shapes: Vec<Shape>,
    slides: Vec<Slide>,
    /// Without bytes; they follow the JSON in this order.
    media: Vec<MediaItem>,
    text: Option<TextBody>,
    source_theme: Option<(deckcraft_color::ColorScheme, deckcraft_model::theme::FontScheme)>,
}

/// A stamp for a new copy: nanoseconds since the Unix epoch, 1 when the clock can't say. A window
/// recognises its own copy by it and keeps its clipboard (paste offsets) rather than re-reading.
pub fn new_stamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1))
}

/// `clip` as clipboard bytes, or `None` when it holds no slides or objects (text goes out as text
/// and HTML only).
pub fn encode(clip: &Clipboard, stamp: u64) -> Option<Vec<u8>> {
    if clip.slides.is_empty() && clip.shapes.is_empty() {
        return None;
    }
    let data = Data {
        stamp,
        plain: clip.plain.clone(),
        shapes: clip.shapes.clone(),
        slides: clip.slides.clone(),
        media: clip.media.clone(),
        text: clip.text.clone(),
        source_theme: clip.source_theme.clone(),
    };
    let json = serde_json::to_vec(&data).ok()?;
    let mut out = Vec::with_capacity(json.len() + 16 + clip.media.iter().map(|m| m.data.len() + 8).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(json.len() as u64).to_le_bytes());
    out.extend_from_slice(&json);
    for m in &clip.media {
        out.extend_from_slice(&(m.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&m.data);
    }
    Some(out)
}

/// Split `n` bytes off the front of `rest`.
fn take<'a>(rest: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if rest.len() < n {
        return None;
    }
    let (head, tail) = rest.split_at(n);
    *rest = tail;
    Some(head)
}

fn take_len(rest: &mut &[u8]) -> Option<usize> {
    let b: [u8; 8] = take(rest, 8)?.try_into().ok()?;
    usize::try_from(u64::from_le_bytes(b)).ok()
}

/// The clipboard and its stamp from bytes [`encode`] made; `None` for anything else (another
/// version, truncated or damaged data).
pub fn decode(bytes: &[u8]) -> Option<(u64, Clipboard)> {
    let mut rest = bytes;
    if take(&mut rest, MAGIC.len())? != MAGIC {
        return None;
    }
    let n = take_len(&mut rest)?;
    let data: Data = serde_json::from_slice(take(&mut rest, n)?).ok()?;
    let mut media = data.media;
    for m in &mut media {
        let n = take_len(&mut rest)?;
        m.data = Arc::new(take(&mut rest, n)?.to_vec());
    }
    let clip =
        Clipboard { shapes: data.shapes, slides: data.slides, media, text: data.text, plain: data.plain, source_theme: data.source_theme, pastes: 0 };
    Some((data.stamp, clip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;

    #[test]
    fn copied_slides_survive_the_round_trip_and_paste_in_another_deck() {
        let mut a = Session::with_new();
        let png = deckcraft_render::Image { width: 4, height: 4, pixels: [255, 0, 0, 255].repeat(16) }.to_png();
        a.execute("insert.picture", &json!({"name": "p.png", "data": crate::cmd::base64_encode(&png)})).unwrap();
        a.execute("edit.copy", &json!({"scope": "slides"})).unwrap();
        let bytes = encode(&a.clipboard, 7).unwrap();
        let (stamp, clip) = decode(&bytes).unwrap();
        assert_eq!(stamp, 7);
        assert_eq!(clip.slides, a.clipboard.slides);
        assert!(!clip.media.is_empty());
        assert_eq!(
            clip.media.iter().map(|m| m.data.clone()).collect::<Vec<_>>(),
            a.clipboard.media.iter().map(|m| m.data.clone()).collect::<Vec<_>>()
        );
        let mut b = Session::with_new();
        b.clipboard = clip;
        let before = b.doc().unwrap().doc.slides.len();
        b.execute("edit.paste", &json!({})).unwrap();
        assert_eq!(b.doc().unwrap().doc.slides.len(), before + 1);
    }

    #[test]
    fn text_has_no_object_bytes_and_damaged_bytes_are_refused() {
        assert!(encode(&Clipboard { plain: "words".into(), ..Default::default() }, 1).is_none());
        let mut a = Session::with_new();
        a.execute("edit.copy", &json!({"scope": "slides"})).unwrap();
        let bytes = encode(&a.clipboard, 1).unwrap();
        for cut in [0, 4, 8, 12, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_none(), "cut at {cut}");
        }
        assert!(decode(b"something else entirely").is_none());
    }
}
