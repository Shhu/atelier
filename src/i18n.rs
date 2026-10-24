//! UI translations. Each language has its own Fluent file in `locales/`
//! (same keys, in the same order), embedded in the executable. English is the
//! fallback: a key missing from a translation is shown in English.
//!
//! In code, `t!("key")` or `t!("key", name = value)` gives the text in the
//! chosen language.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Fr,
}

impl Lang {
    /// English first: it is the fallback language.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Fr];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    /// Name of the language in that language, for the menu.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Français",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Lang::En => include_str!("../locales/en.ftl"),
            Lang::Fr => include_str!("../locales/fr.ftl"),
        }
    }

    /// Accepts "fr", "fr-CA", "fr_FR.UTF-8"…
    fn from_code(code: &str) -> Option<Lang> {
        let primary = code.split(['-', '_', '.']).next()?.to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

    fn index(self) -> usize {
        Lang::ALL
            .iter()
            .position(|&l| l == self)
            .expect("known language")
    }
}

#[cfg(not(test))]
static CURRENT: AtomicUsize = AtomicUsize::new(0);

// Tests run in parallel: each one has its own language.
#[cfg(test)]
thread_local! {
    static CURRENT: AtomicUsize = const { AtomicUsize::new(0) };
}

// A language for this thread only, for a while (see `in_lang`).
thread_local! {
    static OVERRIDE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn current() -> usize {
    if let Some(index) = OVERRIDE.with(std::cell::Cell::get) {
        return index;
    }
    #[cfg(not(test))]
    let index = CURRENT.load(Ordering::Relaxed);
    #[cfg(test)]
    let index = CURRENT.with(|c| c.load(Ordering::Relaxed));
    index
}

/// `f`'s texts in `lang`, whatever the interface's language (the English
/// diagnostics report). Only this thread is affected.
pub fn in_lang<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    let before = OVERRIDE.with(|o| o.replace(Some(lang.index())));
    // Restored even if `f` panics.
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.with(|o| o.set(self.0));
        }
    }
    let _restore = Restore(before);
    f()
}

static BUNDLES: LazyLock<Vec<FluentBundle<FluentResource>>> =
    LazyLock::new(|| Lang::ALL.iter().map(|&l| bundle(l)).collect());

fn bundle(lang: Lang) -> FluentBundle<FluentResource> {
    let id = lang.code().parse().expect("valid language code");
    let mut bundle = FluentBundle::new_concurrent(vec![id]);
    // No Unicode isolation marks around values: egui's fonts show them as
    // empty boxes.
    bundle.set_use_isolating(false);
    // Syntax errors are caught by the tests; whatever parses is kept.
    let resource = FluentResource::try_new(lang.source().to_owned()).unwrap_or_else(|(r, _)| r);
    let _ = bundle.add_resource(resource);
    bundle
}

pub fn lang() -> Lang {
    Lang::ALL[current()]
}

pub fn set_lang(lang: Lang) {
    #[cfg(not(test))]
    CURRENT.store(lang.index(), Ordering::Relaxed);
    #[cfg(test)]
    CURRENT.with(|c| c.store(lang.index(), Ordering::Relaxed));
}

/// Language at startup: the one picked in the menu, else the system's, else
/// English.
pub fn init() {
    let saved = std::fs::read_to_string(settings_file()).ok();
    let lang = saved
        .as_deref()
        .map(str::trim)
        .and_then(Lang::from_code)
        .or_else(|| {
            sys_locale::get_locale()
                .as_deref()
                .and_then(Lang::from_code)
        })
        .unwrap_or(Lang::En);
    set_lang(lang);
}

/// Switches language and remembers it for the next launches.
pub fn choose(lang: Lang) {
    set_lang(lang);
    let file = settings_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, lang.code());
}

fn settings_file() -> PathBuf {
    crate::config::dir().join("language")
}

/// Text of key `id` in the chosen language, falling back to English, then to
/// the key itself if it exists nowhere.
pub fn tr(id: &str, args: Option<&FluentArgs>) -> String {
    let bundles = &*BUNDLES;
    let current = &bundles[current()];
    for bundle in [current, &bundles[0]] {
        if let Some(pattern) = bundle.get_message(id).and_then(|m| m.value()) {
            let mut errors = Vec::new();
            return bundle
                .format_pattern(pattern, args, &mut errors)
                .into_owned();
        }
    }
    id.to_owned()
}

/// A message shown in the interface's language, with its English text for
/// the diagnostics' log and report (see `tm!`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub shown: String,
    pub english: String,
}

impl Message {
    pub fn new(shown: String, english: String) -> Self {
        Self { shown, english }
    }

    /// The text `f` makes with `t!` (an error's `to_string`, a filter's
    /// name), in the interface's language and in English.
    pub fn of(f: impl Fn() -> String) -> Self {
        Self::new(f(), in_lang(Lang::En, &f))
    }
}

/// A text with no English of its own (already English, or not known):
/// the same in both.
impl From<String> for Message {
    fn from(shown: String) -> Self {
        Self {
            english: shown.clone(),
            shown,
        }
    }
}

/// The text shown.
impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.shown)
    }
}

impl From<Message> for String {
    fn from(message: Message) -> Self {
        message.shown
    }
}

impl From<&String> for Message {
    fn from(shown: &String) -> Self {
        shown.clone().into()
    }
}

impl From<&str> for Message {
    fn from(shown: &str) -> Self {
        shown.to_owned().into()
    }
}

/// Text of key `id` in English (the arguments as given).
pub fn tr_en(id: &str, args: Option<&FluentArgs>) -> String {
    in_lang(Lang::En, || tr(id, args))
}

/// `value` with `decimals` digits, written with the language's decimal
/// separator ("1.5" in English, "1,5" in French).
pub fn decimal(value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}").replace('.', &t!("number-decimal-separator"))
}

/// `value` with at most one decimal: "48", "48.5" ("48,5" in French).
pub fn short_decimal(value: f64) -> String {
    let tenths = (value * 10.0).round();
    if tenths == 0.0 {
        "0".to_owned()
    } else if tenths % 10.0 == 0.0 {
        format!("{:.0}", tenths / 10.0)
    } else {
        decimal(tenths / 10.0, 1)
    }
}

/// A number typed by the user: either decimal separator is accepted; "nan"
/// and "inf" are not numbers.
pub fn parse_decimal(text: &str) -> Option<f64> {
    let value: f64 = text.trim().replace(',', ".").parse().ok()?;
    value.is_finite().then_some(value)
}

/// A percentage, `value` already formatted: "50%" in English, "50 %" in
/// French (no-break space).
pub fn percent(value: impl std::fmt::Display) -> String {
    t!("number-percent", value = value.to_string())
}

/// `t!("key")`, or `t!("key", name = value, …)` for the text's `{ $name }`.
/// Numbers passed as is select plural forms; numbers that need precise
/// formatting are passed already formatted, as strings.
macro_rules! t {
    ($id:literal) => {
        $crate::i18n::tr($id, None)
    };
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut args = fluent_bundle::FluentArgs::new();
        $(args.set(stringify!($name), $value);)+
        $crate::i18n::tr($id, Some(&args))
    }};
}
pub(crate) use t;

/// `tm!("key", name = value, …)`: like `t!`, a `Message` with the English
/// text too (the arguments are evaluated once). An argument that is itself
/// a `Message` (an error, a filter's name) gives each text its own.
macro_rules! tm {
    ($id:literal) => {
        $crate::i18n::Message::new($crate::i18n::tr($id, None), $crate::i18n::tr_en($id, None))
    };
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut shown = fluent_bundle::FluentArgs::new();
        let mut english = fluent_bundle::FluentArgs::new();
        $({
            let value = $value;
            shown.set(stringify!($name), $crate::i18n::MessageArg::shown(&value));
            english.set(stringify!($name), $crate::i18n::MessageArg::english(&value));
        })+
        $crate::i18n::Message::new(
            $crate::i18n::tr($id, Some(&shown)),
            $crate::i18n::tr_en($id, Some(&english)),
        )
    }};
}

/// A value given to `tm!`: the same in both texts, or a `Message`'s two.
pub trait MessageArg {
    fn shown(&self) -> fluent_bundle::FluentValue<'static>;
    fn english(&self) -> fluent_bundle::FluentValue<'static> {
        self.shown()
    }
}

impl MessageArg for Message {
    fn shown(&self) -> fluent_bundle::FluentValue<'static> {
        self.shown.clone().into()
    }
    fn english(&self) -> fluent_bundle::FluentValue<'static> {
        self.english.clone().into()
    }
}

impl MessageArg for &Message {
    fn shown(&self) -> fluent_bundle::FluentValue<'static> {
        (*self).shown()
    }
    fn english(&self) -> fluent_bundle::FluentValue<'static> {
        (*self).english()
    }
}

impl MessageArg for String {
    fn shown(&self) -> fluent_bundle::FluentValue<'static> {
        self.clone().into()
    }
}

impl MessageArg for &String {
    fn shown(&self) -> fluent_bundle::FluentValue<'static> {
        (*self).clone().into()
    }
}

impl MessageArg for &str {
    fn shown(&self) -> fluent_bundle::FluentValue<'static> {
        self.to_string().into()
    }
}

macro_rules! number_args {
    ($($t:ty),+) => {$(
        impl MessageArg for $t {
            fn shown(&self) -> fluent_bundle::FluentValue<'static> {
                (*self).into()
            }
        }
    )+};
}
number_args!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);
pub(crate) use tm;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use fluent_syntax::ast;

    use super::*;

    /// Keys and variables (`$name`) of each message in a translation file.
    fn messages(lang: Lang) -> BTreeMap<String, BTreeSet<String>> {
        let resource = fluent_syntax::parser::parse(lang.source())
            .unwrap_or_else(|(_, errors)| panic!("{}: {errors:?}", lang.code()));
        let mut out = BTreeMap::new();
        for entry in resource.body {
            if let ast::Entry::Message(m) = entry {
                let mut vars = BTreeSet::new();
                if let Some(value) = &m.value {
                    collect_vars(value, &mut vars);
                }
                let key = m.id.name.to_owned();
                assert!(
                    out.insert(key.clone(), vars).is_none(),
                    "{}: duplicate key {key}",
                    lang.code()
                );
            }
        }
        out
    }

    fn collect_vars(pattern: &ast::Pattern<&str>, vars: &mut BTreeSet<String>) {
        for element in &pattern.elements {
            if let ast::PatternElement::Placeable { expression } = element {
                collect_expression(expression, vars);
            }
        }
    }

    fn collect_expression(expression: &ast::Expression<&str>, vars: &mut BTreeSet<String>) {
        match expression {
            ast::Expression::Select { selector, variants } => {
                collect_inline(selector, vars);
                for v in variants {
                    collect_vars(&v.value, vars);
                }
            }
            ast::Expression::Inline(inline) => collect_inline(inline, vars),
        }
    }

    fn collect_inline(inline: &ast::InlineExpression<&str>, vars: &mut BTreeSet<String>) {
        match inline {
            ast::InlineExpression::VariableReference { id } => {
                vars.insert(id.name.to_owned());
            }
            ast::InlineExpression::Placeable { expression } => {
                collect_expression(expression, vars);
            }
            _ => {}
        }
    }

    /// Keys passed to `t!` in the code (tests excluded).
    fn used_keys() -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        let mut dirs = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).expect("source directory") {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("readable source");
                let code: String = without_test_module(&source)
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                // `t!(` (or `tm!(`) but not `format!(`; rustfmt may put the
                // key on the next line.
                for macro_call in ["t!(", "tm!("] {
                    for (start, _) in code.match_indices(macro_call) {
                        let before = code[..start].chars().next_back();
                        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue;
                        }
                        let rest = code[start + macro_call.len()..].trim_start();
                        if let Some(rest) = rest.strip_prefix('"') {
                            keys.insert(rest.split('"').next().unwrap_or_default().to_owned());
                        }
                    }
                }
            }
        }
        keys
    }

    /// The source before its inline test module (`#[cfg(test)] mod … {`);
    /// other test-only items (`#[cfg(test)] mod x;`, helpers) don't cut it.
    fn without_test_module(source: &str) -> &str {
        let mut from = 0;
        while let Some(i) = source[from..].find("#[cfg(test)]") {
            let at = from + i;
            let rest = source[at + "#[cfg(test)]".len()..].trim_start();
            let inline_module = rest
                .strip_prefix("mod ")
                .and_then(|r| r.find([';', '{']).map(|j| r.as_bytes()[j] == b'{'));
            if inline_module == Some(true) {
                return &source[..at];
            }
            from = at + 1;
        }
        source
    }

    /// A message keeps its English (the report's), and `in_lang` changes
    /// the language for a while only.
    #[test]
    fn messages_carry_their_english() {
        set_lang(Lang::Fr);
        let m = tm!("menu-file");
        assert_eq!((m.shown.as_str(), m.english.as_str()), ("Fichier", "File"));
        assert_eq!(in_lang(Lang::En, || t!("menu-file")), "File");
        assert_eq!(t!("menu-file"), "Fichier");
        set_lang(Lang::En);
    }

    #[test]
    fn translations_have_the_same_keys_and_variables() {
        let en = messages(Lang::En);
        for lang in &Lang::ALL[1..] {
            let other = messages(*lang);
            let missing: Vec<_> = en.keys().filter(|k| !other.contains_key(*k)).collect();
            let extra: Vec<_> = other.keys().filter(|k| !en.contains_key(*k)).collect();
            assert!(
                missing.is_empty(),
                "{}: missing keys {missing:?}",
                lang.code()
            );
            assert!(extra.is_empty(), "{}: unknown keys {extra:?}", lang.code());
            for (key, vars) in &en {
                assert_eq!(vars, &other[key], "{}: variables of {key}", lang.code());
            }
        }
    }

    #[test]
    fn every_key_is_translated_and_used() {
        let en = messages(Lang::En);
        let used = used_keys();
        let undefined: Vec<_> = used.iter().filter(|k| !en.contains_key(*k)).collect();
        let unused: Vec<_> = en.keys().filter(|k| !used.contains(*k)).collect();
        assert!(
            undefined.is_empty(),
            "keys missing from en.ftl: {undefined:?}"
        );
        assert!(unused.is_empty(), "unused keys: {unused:?}");
    }

    /// French puts a no-break space before : ; ! ? % and inside « »: a
    /// line must not start with one of them. English has no space before %.
    #[test]
    fn typography_follows_the_language() {
        let lines = |lang: Lang| {
            lang.source()
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
        };
        for line in lines(Lang::Fr) {
            for bad in [" :", " ;", " !", " ?", " %", "« ", " »"] {
                assert!(
                    !line.contains(bad),
                    "fr: breakable space in {bad:?}: {line}"
                );
            }
        }
        for line in lines(Lang::En) {
            assert!(!line.contains(" %"), "en: space before %: {line}");
        }
        // Straight apostrophes and quotes everywhere (French quotes with « »).
        for lang in Lang::ALL {
            for line in lines(lang) {
                assert!(
                    !line.contains(['’', '“', '”']),
                    "{}: curly quote in {line}",
                    lang.code()
                );
            }
        }
    }

    /// French "one" also covers 0: its variants show the number rather than
    /// a written "1" ("annulé après 1 document" when none was processed).
    #[test]
    fn french_singular_variants_show_the_count() {
        fn check(pattern: &ast::Pattern<&str>, key: &str) {
            for element in &pattern.elements {
                if let ast::PatternElement::Placeable {
                    expression: ast::Expression::Select { variants, .. },
                } = element
                {
                    for v in variants {
                        if matches!(v.key, ast::VariantKey::Identifier { name: "one" }) {
                            let mut vars = BTreeSet::new();
                            collect_vars(&v.value, &mut vars);
                            assert!(!vars.is_empty(), "fr: {key}: [one] without its number");
                        }
                        check(&v.value, key);
                    }
                }
            }
        }
        let resource = fluent_syntax::parser::parse(Lang::Fr.source()).expect("valid fr.ftl");
        for entry in resource.body {
            if let ast::Entry::Message(m) = entry
                && let Some(value) = &m.value
            {
                check(value, m.id.name);
            }
        }
    }

    #[test]
    fn numbers_follow_the_language() {
        set_lang(Lang::Fr);
        assert_eq!(decimal(1.4, 2), "1,40");
        assert_eq!(short_decimal(7.5), "7,5");
        assert_eq!(short_decimal(48.0), "48");
        assert_eq!(percent(50), "50\u{a0}%");
        set_lang(Lang::En);
        assert_eq!(decimal(1.4, 2), "1.40");
        assert_eq!(short_decimal(-0.01), "0");
        assert_eq!(percent("7.5"), "7.5%");
        // The suffix of a percent field.
        assert_eq!(percent(""), "%");
        set_lang(Lang::Fr);
        assert_eq!(percent(""), "\u{a0}%");
        set_lang(Lang::En);
    }

    #[test]
    fn typed_numbers_accept_both_separators_and_reject_nan() {
        assert_eq!(parse_decimal(" 1,5 "), Some(1.5));
        assert_eq!(parse_decimal("1.5"), Some(1.5));
        assert_eq!(parse_decimal("nan"), None);
        assert_eq!(parse_decimal("inf"), None);
        assert_eq!(parse_decimal("abc"), None);
    }

    #[test]
    fn recognizes_system_locales() {
        assert_eq!(Lang::from_code("fr-FR"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("fr_CA.UTF-8"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("en-US"), Some(Lang::En));
        assert_eq!(Lang::from_code("de-DE"), None);
    }
}
