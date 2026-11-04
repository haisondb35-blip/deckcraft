//! Interface translations, as in WordCraft and GridCraft. Command ids, document text and saved
//! files stay as they are; only the interface chrome is translated, and a label without a
//! translation stays English, so coverage can grow a catalog at a time.
//!
//! The interface language is a preference ([`crate::UiState::language`]): empty means Auto, the
//! system's language ([`Language::system`]), else a code (`vi`, `de`…). File › Options › General
//! changes it (`options.set {language}`); the app sets the language each frame
//! ([`set_current`]) and the shared widgets ([`crate::widgets`], the ribbon, the File tab)
//! translate their labels with [`tr`].
//!
//! Catalogs are `i18n/<code>.tsv`, in craft-i18n's format: `context <TAB> English <TAB>
//! translation`, `#` comments, `\n` `\t` `\\` escapes. Their rows come from the other Craft apps'
//! catalogs where DeckCraft shows the same English, plus DeckCraft's own.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

/// An interface language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    #[default]
    En,
    Cs,
    De,
    El,
    Es,
    Fr,
    It,
    Ja,
    Ko,
    Nl,
    Pl,
    PtBr,
    Ro,
    Ru,
    Sr,
    Tr,
    Uk,
    Vi,
    Zh,
}

impl Language {
    pub const ALL: [Self; 19] = [
        Self::En,
        Self::Cs,
        Self::De,
        Self::El,
        Self::Es,
        Self::Fr,
        Self::It,
        Self::Ja,
        Self::Ko,
        Self::Nl,
        Self::Pl,
        Self::PtBr,
        Self::Ro,
        Self::Ru,
        Self::Sr,
        Self::Tr,
        Self::Uk,
        Self::Vi,
        Self::Zh,
    ];

    /// The language's own name, as the picker lists it.
    pub fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Cs => "Čeština",
            Self::De => "Deutsch",
            Self::El => "Ελληνικά",
            Self::Es => "Español",
            Self::Fr => "Français",
            Self::It => "Italiano",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
            Self::Nl => "Nederlands",
            Self::Pl => "Polski",
            Self::PtBr => "Português (Brasil)",
            Self::Ro => "Română",
            Self::Ru => "Русский",
            Self::Sr => "Srpski",
            Self::Tr => "Türkçe",
            Self::Uk => "Українська",
            Self::Vi => "Tiếng Việt",
            Self::Zh => "简体中文",
        }
    }

    /// The code saved in the preferences and taken by `options.set {language}`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Cs => "cs",
            Self::De => "de",
            Self::El => "el",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::It => "it",
            Self::Ja => "ja",
            Self::Ko => "ko",
            Self::Nl => "nl",
            Self::Pl => "pl",
            Self::PtBr => "pt",
            Self::Ro => "ro",
            Self::Ru => "ru",
            Self::Sr => "sr",
            Self::Tr => "tr",
            Self::Uk => "uk",
            Self::Vi => "vi",
            Self::Zh => "zh",
        }
    }

    /// A language from a code or a locale tag (`vi`, `vi-VN`, `pt_BR`); `None` when DeckCraft has
    /// no such interface language.
    pub fn parse(tag: &str) -> Option<Self> {
        let primary = tag.trim().split(['-', '_', '.']).next()?.to_ascii_lowercase();
        Self::ALL.into_iter().find(|l| l.code() == primary)
    }

    /// The saved preference as a language: empty or unknown means the system's language.
    pub fn from_pref(pref: &str) -> Self {
        if pref.trim().is_empty() { Self::system() } else { Self::parse(pref).unwrap_or_else(Self::system) }
    }

    /// The system's interface language, best effort; English when unknown or not offered.
    pub fn system() -> Self {
        static SYSTEM: OnceLock<Language> = OnceLock::new();
        *SYSTEM.get_or_init(|| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                sys_locale::get_locale().and_then(|tag| Self::parse(&tag)).unwrap_or(Self::En)
            }
            #[cfg(target_arch = "wasm32")]
            {
                Self::En
            }
        })
    }

    /// The picker's order: English (the source language) first, then the rest by their own
    /// names with accents folded (Čeština under C), other scripts after the Latin ones.
    pub fn for_picker() -> Vec<Self> {
        let mut rest: Vec<Self> = Self::ALL.into_iter().filter(|l| *l != Self::En).collect();
        rest.sort_by_cached_key(|l| {
            let name = l.name();
            let latin = name.chars().next().is_some_and(|c| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c));
            (!latin, fold(name))
        });
        std::iter::once(Self::En).chain(rest).collect()
    }

    /// `text` in this language, or `text` itself when the catalog has no row for it.
    pub fn tr(self, text: &str) -> &str {
        catalog(self).and_then(|c| c.get(text).copied()).unwrap_or(text)
    }

    /// `text` in this language under `context` (two meanings of one English word).
    pub fn tr_in<'a>(self, context: &str, text: &'a str) -> &'a str {
        catalog(self).and_then(|c| c.get(format!("{context}\u{4}{text}").as_str()).copied()).unwrap_or(text)
    }

    /// How many rows the catalog has (0 for English).
    pub fn rows(self) -> usize {
        catalog(self).map_or(0, HashMap::len)
    }
}

/// Lower case with Latin accents dropped, for sorting names.
fn fold(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'Č' | 'č' => 'c',
            'É' | 'é' | 'È' | 'è' | 'Ê' | 'ê' => 'e',
            'Í' | 'í' => 'i',
            'Ó' | 'ó' | 'Ô' | 'ô' => 'o',
            'Ü' | 'ü' | 'Ú' | 'ú' => 'u',
            'Ă' | 'ă' | 'Â' | 'â' | 'Á' | 'á' => 'a',
            'Ș' | 'ș' | 'Š' | 'š' => 's',
            'Ţ' | 'ţ' | 'Ț' | 'ț' => 't',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The catalog file of each language (English has none).
fn source(lang: Language) -> Option<&'static str> {
    Some(match lang {
        Language::En => return None,
        Language::Cs => include_str!("i18n/cs.tsv"),
        Language::De => include_str!("i18n/de.tsv"),
        Language::El => include_str!("i18n/el.tsv"),
        Language::Es => include_str!("i18n/es.tsv"),
        Language::Fr => include_str!("i18n/fr.tsv"),
        Language::It => include_str!("i18n/it.tsv"),
        Language::Ja => include_str!("i18n/ja.tsv"),
        Language::Ko => include_str!("i18n/ko.tsv"),
        Language::Nl => include_str!("i18n/nl.tsv"),
        Language::Pl => include_str!("i18n/pl.tsv"),
        Language::PtBr => include_str!("i18n/pt.tsv"),
        Language::Ro => include_str!("i18n/ro.tsv"),
        Language::Ru => include_str!("i18n/ru.tsv"),
        Language::Sr => include_str!("i18n/sr.tsv"),
        Language::Tr => include_str!("i18n/tr.tsv"),
        Language::Uk => include_str!("i18n/uk.tsv"),
        Language::Vi => include_str!("i18n/vi.tsv"),
        Language::Zh => include_str!("i18n/zh.tsv"),
    })
}

type Catalog = HashMap<&'static str, &'static str>;

/// A catalog read once, on first use. Rows with escapes are leaked once, so lookups borrow.
fn catalog(lang: Language) -> Option<&'static Catalog> {
    static CATALOGS: [OnceLock<Catalog>; 19] = [const { OnceLock::new() }; 19];
    let i = Language::ALL.iter().position(|l| *l == lang)?;
    let text = source(lang)?;
    CATALOGS.get(i).map(|slot| slot.get_or_init(|| parse(text)))
}

/// Read a catalog: `context <TAB> source <TAB> translation` rows; an empty translation, or a row
/// whose placeholders (`{name}`) differ from the source's, is left out (English shows).
pub fn parse(text: &'static str) -> Catalog {
    let mut out = Catalog::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let (Some(context), Some(source), Some(target)) = (cols.next(), cols.next(), cols.next()) else { continue };
        if source.is_empty() || target.is_empty() || placeholders(source) != placeholders(target) {
            continue;
        }
        let key: &'static str = match (context.is_empty(), source.contains('\\')) {
            (true, false) => source,
            (true, true) => Box::leak(unescape(source).into_boxed_str()),
            (false, _) => Box::leak(format!("{context}\u{4}{}", unescape(source)).into_boxed_str()),
        };
        let value: &'static str = if target.contains('\\') { Box::leak(unescape(target).into_boxed_str()) } else { target };
        out.entry(key).or_insert(value);
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The `{name}` placeholders in `s`, sorted.
fn placeholders(s: &str) -> Vec<&str> {
    let mut out: Vec<&str> = s.match_indices('{').filter_map(|(i, _)| s.get(i..).and_then(|r| r.find('}').and_then(|j| r.get(..=j)))).collect();
    out.sort_unstable();
    out
}

thread_local! {
    static CURRENT: Cell<Language> = const { Cell::new(Language::En) };
}

/// The interface language of this thread's window (English until the app sets it).
pub fn current() -> Language {
    CURRENT.with(Cell::get)
}

/// Set the interface language (the app does it at the start of every frame).
pub fn set_current(lang: Language) {
    CURRENT.with(|c| c.set(lang));
}

/// `text` in the current interface language, or `text` when it has no translation.
pub fn tr(text: &str) -> &str {
    current().tr(text)
}

/// [`tr`] for text built at run time.
pub fn tr_owned(text: &str) -> String {
    tr(text).to_string()
}

/// [`tr`] for a ribbon label, which may be broken over two lines (`Ink to\nShape`): the
/// catalog has it on one line, and the button breaks the translation again.
pub fn tr_label(text: &str) -> std::borrow::Cow<'_, str> {
    let lang = current();
    if lang == Language::En || text.is_empty() {
        return std::borrow::Cow::Borrowed(text);
    }
    let Some(c) = catalog(lang) else { return std::borrow::Cow::Borrowed(text) };
    if let Some(hit) = c.get(text) {
        return std::borrow::Cow::Borrowed(hit);
    }
    if text.contains('\n')
        && let Some(hit) = c.get(text.replace('\n', " ").as_str())
    {
        return std::borrow::Cow::Borrowed(hit);
    }
    std::borrow::Cow::Borrowed(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_tags_read_back() {
        for l in Language::ALL {
            assert_eq!(Language::parse(l.code()), Some(l));
        }
        assert_eq!(Language::parse("vi-VN"), Some(Language::Vi));
        assert_eq!(Language::parse("pt_BR"), Some(Language::PtBr));
        assert_eq!(Language::parse("xx"), None);
        assert_eq!(Language::from_pref("vi"), Language::Vi);
        assert_eq!(Language::for_picker().first(), Some(&Language::En));
        assert_eq!(Language::for_picker().len(), Language::ALL.len());
    }

    #[test]
    fn rows_escapes_contexts_and_placeholders() {
        let c = parse("# comment\n\tPaste\tDán\n\tLine\\nTwo\tDòng\\nHai\nmenu\tOrder\tThứ tự\n\tSlide {n}\tTrang {m}\n\tEmpty\t\n");
        assert_eq!(c.get("Paste"), Some(&"Dán"));
        assert_eq!(c.get("Line\nTwo"), Some(&"Dòng\nHai"));
        assert_eq!(c.get("menu\u{4}Order"), Some(&"Thứ tự"));
        assert!(!c.contains_key("Slide {n}"), "a placeholder mismatch is dropped");
        assert!(!c.contains_key("Empty"));
    }

    #[test]
    fn every_catalog_reads_and_vietnamese_covers_the_ribbon() {
        for l in Language::ALL.into_iter().filter(|l| *l != Language::En) {
            assert!(l.rows() > 100, "{l:?} has {} rows", l.rows());
        }
        let vi = Language::Vi;
        for (en, want) in [("Paste", "Dán"), ("New Slide", "Trang chiếu mới"), ("Home", "Trang đầu")] {
            assert_eq!(vi.tr(en), want, "{en}");
        }
        assert_eq!(vi.tr("No such label anywhere"), "No such label anywhere");
        assert_eq!(Language::En.tr("Paste"), "Paste");
    }

    #[test]
    fn the_current_language_is_per_thread() {
        set_current(Language::Vi);
        assert_eq!(tr("Paste"), "Dán");
        std::thread::spawn(|| assert_eq!(tr("Paste"), "Paste")).join().unwrap();
        set_current(Language::En);
        assert_eq!(tr("Paste"), "Paste");
    }
}
