use std::sync::Arc;

use super::*;
use deckcraft_model::text::TextBody;

fn deck(n: usize) -> Presentation {
    let mut p = deckcraft_model::defaults::new_presentation(None);
    let layout = p.slides[0].layout;
    while p.slides.len() < n {
        let s = deckcraft_model::defaults::new_slide(&mut p, layout).expect("slide");
        p.slides.push(Arc::new(s));
    }
    for (i, s) in p.slides.iter_mut().enumerate() {
        let s = Arc::make_mut(s);
        if let Some(sh) = s.shapes.first_mut() {
            sh.text = Some(TextBody::from_text(&format!("Slide title {i}")));
        }
        s.notes = TextBody::from_text("Speaker notes that wrap across the page width to test line breaking of long notes.");
    }
    p
}

fn opts(layout: PageLayout) -> PdfOptions {
    PdfOptions { layout, dpi: 48.0, ..Default::default() }
}

#[test]
fn exports_slides_notes_and_handouts() {
    let p = deck(5);
    for layout in [PageLayout::Slides, PageLayout::Notes, PageLayout::Handouts { per_page: 3 }, PageLayout::Handouts { per_page: 6 }] {
        let bytes = export(&p, &opts(layout)).expect("export");
        assert!(bytes.starts_with(b"%PDF-"), "{layout:?}");
        assert!(bytes.len() > 1000);
    }
}

#[test]
fn empty_range_is_an_error() {
    let p = deck(1);
    let o = PdfOptions { slides: Some(vec![7]), ..opts(PageLayout::Slides) };
    assert!(matches!(export(&p, &o), Err(PdfError::NoPages)));
}

#[test]
fn hidden_slides_skipped_unless_asked() {
    let mut p = deck(2);
    Arc::make_mut(&mut p.slides[1]).hidden = true;
    let a = export(&p, &opts(PageLayout::Slides)).unwrap();
    let b = export(&p, &PdfOptions { include_hidden: true, ..opts(PageLayout::Slides) }).unwrap();
    assert!(b.len() > a.len());
}

#[test]
fn handout_boxes_fit_the_paper() {
    for n in [1, 2, 3, 4, 6, 9] {
        let v = handout_boxes(n, (612.0, 792.0), 16.0 / 9.0);
        assert_eq!(v.len(), n as usize);
        for r in &v {
            assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= 612.0 && r.y1 <= 792.0, "{n}: {r:?}");
            assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 1e-6);
        }
        for (i, a) in v.iter().enumerate() {
            for b in &v[i + 1..] {
                assert!(a.intersect(*b).area() <= 0.0, "{n}: overlap");
            }
        }
    }
}

/// Notes pages and handouts print on the chosen paper, landscape included, with every slide box
/// on the page and room left for the notes.
#[test]
fn notes_and_handouts_fit_any_paper_and_orientation() {
    let p = deck(3);
    for (id, _, _) in PAPERS {
        for landscape in [false, true] {
            let paper = paper_size(id, landscape).expect("known paper");
            assert_eq!(paper.0 > paper.1, landscape, "{id}");
            let b = notes_slide_box(paper, 16.0 / 9.0);
            assert!(b.x0 >= 0.0 && b.x1 <= paper.0 && b.y1 <= paper.1 * 0.5, "{id} {landscape}: {b:?}");
            assert!((b.width() / b.height() - 16.0 / 9.0).abs() < 1e-6);
            for n in [1, 3, 6, 9] {
                for r in handout_boxes(n, paper, 16.0 / 9.0) {
                    assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= paper.0 && r.y1 <= paper.1, "{id} {landscape} {n}: {r:?}");
                }
            }
            for layout in [PageLayout::Notes, PageLayout::Handouts { per_page: 6 }] {
                let bytes = export(&p, &PdfOptions { paper, ..opts(layout) }).expect("export");
                let text = String::from_utf8_lossy(&bytes);
                let media = format!("/MediaBox[0 0 {} {}]", fmt(paper.0), fmt(paper.1));
                assert!(text.contains(&media), "{id} {landscape} {layout:?}: no {media}");
            }
        }
    }
    assert_eq!(paper_size("bogus", false), None);
}

fn fmt(v: f64) -> String {
    let s = format!("{:.2}", v as f32);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn ph(kind: deckcraft_model::PhType, x: f64, y: f64, w: f64, h: f64, text: &str) -> deckcraft_model::Shape {
    deckcraft_model::Shape {
        xfrm: Some(deckcraft_model::Xfrm::new(x, y, w, h)),
        ph: Some(deckcraft_model::Placeholder { kind, ..Default::default() }),
        text: Some(TextBody::from_text(text)),
        ..Default::default()
    }
}

#[test]
fn notes_pages_and_handouts_follow_their_masters() {
    use deckcraft_model::PhType;
    let mut p = deck(2);
    let paper = (612.0, 792.0);
    // No masters: the default layout (slide at the top, one-inch side margins).
    let (slide, notes) = notes_frames(&p, paper);
    assert_eq!((slide.x0, slide.y0), (72.0, 54.0));
    assert!(notes.y0 > slide.y1);
    // A notes master moves the slide and the notes, scaled from the notes size to the paper.
    p.notes_size = deckcraft_model::Size::new(612.0, 792.0);
    p.notes_master = Some(Arc::new(deckcraft_model::Master {
        shapes: vec![
            ph(PhType::SlideImage, 100.0, 400.0, 412.0, 300.0, ""),
            ph(PhType::Body, 50.0, 50.0, 500.0, 300.0, ""),
            ph(PhType::Header, 0.0, 0.0, 300.0, 30.0, "Q3 review"),
            ph(PhType::Date, 312.0, 0.0, 300.0, 30.0, ""),
            ph(PhType::SlideNum, 312.0, 760.0, 300.0, 30.0, "<#>"),
        ],
        ..Default::default()
    }));
    let (slide, notes) = notes_frames(&p, paper);
    assert!(slide.y0 >= 400.0 && slide.y1 <= 700.0 + 1e-6 && slide.x0 >= 100.0 - 1e-6, "{slide:?}");
    assert!(((slide.width() / slide.height()) - p.slide_size.width / p.slide_size.height).abs() < 1e-6);
    assert_eq!(notes, Rect::new(50.0, 50.0, 550.0, 350.0));
    let texts = master_texts(&p, p.notes_master.as_deref().unwrap(), paper, 7);
    let got: Vec<(&str, bool)> = texts.iter().map(|(_, t, r, _)| (t.as_str(), *r)).collect();
    assert_eq!(got, [("Q3 review", false), ("7", true)], "empty date left out, number is the page");
    // Notes pages and handouts export with the masters.
    for layout in [PageLayout::Notes, PageLayout::Handouts { per_page: 3 }] {
        if layout != PageLayout::Notes {
            p.handout_master = Some(Arc::new(deckcraft_model::Master {
                shapes: vec![ph(PhType::Footer, 0.0, 760.0, 300.0, 30.0, "Confidential")],
                ..Default::default()
            }));
        }
        let bytes = export(&p, &PdfOptions { layout, ..Default::default() }).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }
}

#[test]
fn master_text_keeps_its_own_formatting() {
    use deckcraft_model::text::RunProps;
    use deckcraft_model::{ColorRef, Fill, PhType, Rgba};
    let mut p = deck(1);
    p.notes_size = deckcraft_model::Size::new(612.0, 792.0);
    let mut header = ph(PhType::Header, 0.0, 0.0, 300.0, 30.0, "Board pack");
    if let Some(run) = header.text.as_mut().and_then(|t| t.paragraphs.first_mut()).and_then(|p| p.runs.first_mut()) {
        run.props = RunProps {
            size: Some(20.0),
            bold: Some(true),
            fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(200, 0, 0)))),
            font: Some("+mj-lt".into()),
            ..Default::default()
        };
    }
    let m = deckcraft_model::Master { shapes: vec![header, ph(PhType::Footer, 0.0, 760.0, 300.0, 30.0, "plain")], ..Default::default() };
    let texts = master_texts(&p, &m, (612.0, 792.0), 1);
    let st = &texts[0].3;
    assert_eq!((st.size, st.bold, st.italic, st.color), (20.0, true, false, Rgba::rgb(200, 0, 0)));
    assert_eq!(st.family, m.theme.font("+mj-lt"), "theme font resolved");
    // Unstyled text falls back through the master's styles; sizes scale with the paper.
    let half = master_texts(&p, &m, (306.0, 396.0), 1);
    assert!((half[0].3.size - 10.0).abs() < 1e-9);
    assert!(texts[1].3.size > 0.0 && !texts[1].3.bold);
    p.notes_master = Some(Arc::new(m));
    assert!(export(&p, &PdfOptions { layout: PageLayout::Notes, ..Default::default() }).unwrap().starts_with(b"%PDF"));
}

#[test]
fn master_text_wraps_and_keeps_mixed_formatting() {
    use deckcraft_model::text::{Run, RunProps};
    use deckcraft_model::{PhType, Rgba};
    let mut p = deck(1);
    p.notes_size = deckcraft_model::Size::new(612.0, 792.0);
    let mut header = ph(PhType::Header, 0.0, 0.0, 200.0, 80.0, "");
    if let Some(para) = header.text.as_mut().and_then(|t| t.paragraphs.first_mut()) {
        para.runs = vec![
            Run::with("Quarterly board pack ", RunProps { size: Some(18.0), bold: Some(true), ..Default::default() }),
            Run::with(
                "for internal circulation only, please do not forward",
                RunProps {
                    size: Some(9.0),
                    fill: Some(deckcraft_model::Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(200, 0, 0)))),
                    ..Default::default()
                },
            ),
        ];
    }
    let m = deckcraft_model::Master { shapes: vec![header, ph(PhType::SlideNum, 312.0, 760.0, 300.0, 30.0, "")], ..Default::default() };
    let placed = deckcraft_render::place_master_texts(&p, &m, 4);
    assert_eq!(placed.len(), 2, "header and page number");
    let h = &placed[0].layout;
    assert!(h.lines.len() >= 2, "wraps inside its 200 pt placeholder: {} lines", h.lines.len());
    let sizes: Vec<f64> = h.runs.iter().filter(|r| !r.glyphs.is_empty()).map(|r| r.size).collect();
    assert!(sizes.iter().any(|s| (*s - 18.0).abs() < 0.5) && sizes.iter().any(|s| (*s - 9.0).abs() < 0.5), "{sizes:?}");
    assert!(h.runs.iter().any(|r| r.color == Rgba::rgb(200, 0, 0)));
    let num: String = placed[1].body.text();
    assert_eq!(num, "4");
    // The same layout as pieces for the Notes Page view: on more than one line, scaled to paper.
    let pieces = master_text_pieces(&p, &m, (306.0, 396.0), 4);
    let mut ys: Vec<i64> = pieces.iter().map(|x| x.at.1.round() as i64).collect();
    ys.dedup();
    assert!(ys.len() >= 3, "two header lines and the number: {pieces:?}");
    assert!(pieces.iter().any(|x| x.bold && (x.size - 9.0).abs() < 0.5), "18 pt bold at half size");
    assert!(pieces.iter().any(|x| x.color == Rgba::rgb(200, 0, 0) && !x.bold));
    assert!(pieces.iter().any(|x| x.text == "4"));
    p.notes_master = Some(Arc::new(m));
    assert!(export(&p, &PdfOptions { layout: PageLayout::Notes, ..Default::default() }).unwrap().starts_with(b"%PDF"));
}

#[test]
fn glyph_ranges_follow_shaping_clusters() {
    // "fl" ligature: its glyph maps to both characters, the rest stay one to one (#164).
    assert_eq!(glyph_text_ranges("flash", (0, 5), &[(0, 'f'), (2, 'a'), (3, 's'), (4, 'h')], 4), vec![0..2, 2..3, 3..4, 4..5]);
    // Two glyphs in one cluster (base and mark) share its range.
    assert_eq!(glyph_text_ranges("ab", (0, 2), &[(0, 'a'), (0, 'a'), (1, 'b')], 3), vec![0..1, 0..1, 1..2]);
    // A run starting mid-paragraph, multi-byte characters in visual (right-to-left) order.
    assert_eq!(glyph_text_ranges("αβγ", (3, 6), &[(5, 'γ'), (4, 'β'), (3, 'α')], 3), vec![4..6, 2..4, 0..2]);
}
