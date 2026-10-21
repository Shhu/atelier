//! Type layers: their content and styles (rich text: character styles by
//! runs, paragraph styles by paragraph), laid out and rendered with the
//! fonts installed on the system (see [`TextEngine`]).

mod layout;
mod raster;

use std::ops::Range;

pub use layout::{Face, Layout, Rendered, TextEngine};

use crate::i18n::t;

/// Auto leading, as a multiple of the font size.
pub const AUTO_LEADING: f32 = 1.2;
/// Faux small capitals, relative to the capitals.
const SMALL_CAPS: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    /// Justified, the last line aligned left.
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AntiAlias {
    None,
    /// Hinted outlines: crisp at small sizes.
    Sharp,
    /// Exact outlines.
    Smooth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kerning {
    /// The font's kerning pairs.
    Metrics,
    /// No kerning ("0").
    Zero,
}

/// Character attributes (Character panel).
#[derive(Clone, Debug, PartialEq)]
pub struct CharStyle {
    /// Font family name; empty = default sans-serif font.
    pub family: String,
    /// Font style: weight (100–900), italic, width (1–9, 5 = normal).
    pub weight: u16,
    pub italic: bool,
    pub stretch: u16,
    /// Size in pixels.
    pub size: f32,
    /// Distance between baselines, in pixels; `None`: auto.
    pub leading: Option<f32>,
    /// Thousandths of an em added between letters.
    pub tracking: f32,
    pub kerning: Kerning,
    /// Glyph scales (1 = 100 %).
    pub horizontal_scale: f32,
    pub vertical_scale: f32,
    /// Raise (or lower) from the baseline, in pixels.
    pub baseline_shift: f32,
    pub color: [u8; 4],
    pub faux_bold: bool,
    pub faux_italic: bool,
    pub all_caps: bool,
    pub small_caps: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl Default for CharStyle {
    fn default() -> Self {
        Self {
            family: String::new(),
            weight: 400,
            italic: false,
            stretch: 5,
            size: 48.0,
            leading: None,
            tracking: 0.0,
            kerning: Kerning::Metrics,
            horizontal_scale: 1.0,
            vertical_scale: 1.0,
            baseline_shift: 0.0,
            color: [0, 0, 0, 255],
            faux_bold: false,
            faux_italic: false,
            all_caps: false,
            small_caps: false,
            underline: false,
            strikethrough: false,
        }
    }
}

impl CharStyle {
    /// Distance between baselines.
    pub fn line_height(&self) -> f32 {
        self.leading.unwrap_or(self.size * AUTO_LEADING)
    }

    /// Valid values (damaged files, typed values).
    pub fn sanitize(&mut self) {
        let finite = |v: f32, default: f32| if v.is_finite() { v } else { default };
        self.weight = self.weight.clamp(1, 1000);
        self.stretch = self.stretch.clamp(1, 9);
        self.size = finite(self.size, 48.0).clamp(0.1, 5000.0);
        self.leading = self
            .leading
            .filter(|l| l.is_finite())
            .map(|l| l.clamp(0.0, 10_000.0));
        self.tracking = finite(self.tracking, 0.0).clamp(-1000.0, 10_000.0);
        self.horizontal_scale = finite(self.horizontal_scale, 1.0)
            .clamp(0.0, 10.0)
            .max(0.01);
        self.vertical_scale = finite(self.vertical_scale, 1.0).clamp(0.0, 10.0).max(0.01);
        self.baseline_shift = finite(self.baseline_shift, 0.0).clamp(-10_000.0, 10_000.0);
    }
}

/// Paragraph attributes (Paragraph panel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParagraphStyle {
    pub align: TextAlign,
    /// Indents, in pixels; the first line's adds to the left one.
    pub indent_left: f32,
    pub indent_right: f32,
    pub indent_first: f32,
    pub space_before: f32,
    pub space_after: f32,
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        Self {
            align: TextAlign::Left,
            indent_left: 0.0,
            indent_right: 0.0,
            indent_first: 0.0,
            space_before: 0.0,
            space_after: 0.0,
        }
    }
}

impl ParagraphStyle {
    pub fn sanitize(&mut self) {
        for v in [
            &mut self.indent_left,
            &mut self.indent_right,
            &mut self.indent_first,
            &mut self.space_before,
            &mut self.space_after,
        ] {
            *v = if v.is_finite() {
                v.clamp(-10_000.0, 10_000.0)
            } else {
                0.0
            };
        }
    }
}

/// Characters sharing a style: `len` bytes of the text.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub len: usize,
    pub style: CharStyle,
}

/// Content and styles of a type layer.
#[derive(Clone, Debug, PartialEq)]
pub struct TextData {
    pub text: String,
    /// Character styles, covering `text` in order (a single empty run when
    /// the text is empty: the style of what will be typed).
    pub runs: Vec<Run>,
    /// One per paragraph (line of `text`).
    pub paragraphs: Vec<ParagraphStyle>,
    /// Paragraph text: size of its box, where the lines wrap. `None`: point
    /// text, whose lines only break where the text does.
    pub frame: Option<(f32, f32)>,
    pub antialias: AntiAlias,
    /// Linear part of the transformation from the text to the layer
    /// (rotation, scale, skew): (x, y) → (a·x + b·y, c·x + d·y).
    pub transform: [f32; 4],
    /// Where the origin of the text lies in the layer's image: on the
    /// first baseline at the alignment point (point text), or the box's
    /// top-left corner (paragraph text). Set by each rendering.
    pub anchor: (f32, f32),
}

pub const IDENTITY: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

impl Default for TextData {
    fn default() -> Self {
        Self::new(CharStyle::default(), ParagraphStyle::default())
    }
}

impl TextData {
    /// Empty point text that will be typed with these styles.
    pub fn new(style: CharStyle, paragraph: ParagraphStyle) -> Self {
        Self {
            text: String::new(),
            runs: vec![Run { len: 0, style }],
            paragraphs: vec![paragraph],
            frame: None,
            antialias: AntiAlias::Sharp,
            transform: IDENTITY,
            anchor: (0.0, 0.0),
        }
    }

    /// `text` in a single style.
    pub fn plain(text: &str, style: CharStyle) -> Self {
        let mut data = Self::new(style.clone(), ParagraphStyle::default());
        data.insert(0, text, &style);
        data
    }

    /// Layer name derived from the text (first line, shortened).
    pub fn auto_name(&self) -> String {
        let first = self.text.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            return t!("doc-text-layer-default-name");
        }
        let mut name: String = first.chars().take(24).collect();
        if first.chars().count() > 24 {
            name.push('…');
        }
        name
    }

    /// Style of the character before byte `i` (what typing there takes),
    /// or of the first one.
    pub fn style_at(&self, i: usize) -> &CharStyle {
        let mut end = 0;
        for run in &self.runs {
            end += run.len;
            if end >= i && run.len > 0 {
                return &run.style;
            }
        }
        &self.runs[self.runs.len() - 1].style
    }

    /// Styles of the characters of `range` (of the character before it if
    /// it holds none), in order.
    pub fn styles_in(&self, range: Range<usize>) -> Vec<&CharStyle> {
        let mut start = 0;
        let mut out = Vec::new();
        for run in &self.runs {
            let end = start + run.len;
            if end > range.start && start < range.end {
                out.push(&run.style);
            }
            start = end;
        }
        if out.is_empty() {
            out.push(self.style_at(range.start));
        }
        out
    }

    /// Inserts `s` at byte `at` in `style`. New paragraphs take the style
    /// of the one they split.
    pub fn insert(&mut self, at: usize, s: &str, style: &CharStyle) {
        if s.is_empty() {
            return;
        }
        let p = self.paragraph_at(at);
        let breaks = s.matches('\n').count();
        let para = self.paragraphs[p];
        self.paragraphs
            .splice(p + 1..p + 1, std::iter::repeat_n(para, breaks));
        self.text.insert_str(at, s);
        let i = self.split_runs(at);
        self.runs.insert(
            i,
            Run {
                len: s.len(),
                style: style.clone(),
            },
        );
        self.merge_runs();
    }

    /// Removes the bytes of `range`. Joined paragraphs keep the style of
    /// the first one.
    pub fn delete(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let p = self.paragraph_at(range.start);
        let breaks = self.text[range.clone()].matches('\n').count();
        self.paragraphs.drain(p + 1..p + 1 + breaks);
        // Emptied, the text keeps the style of its first character to type
        // with.
        let keep = self.style_at(range.start + 1).clone();
        self.text.replace_range(range.clone(), "");
        let a = self.split_runs(range.start);
        let b = self.split_runs(range.end);
        self.runs.drain(a..b);
        if self.runs.is_empty() {
            self.runs.push(Run {
                len: 0,
                style: keep,
            });
        }
        self.merge_runs();
    }

    /// Applies `f` to the styles of `range`; on an empty text, to the style
    /// of what will be typed.
    pub fn restyle(&mut self, range: Range<usize>, f: impl Fn(&mut CharStyle)) {
        if self.text.is_empty() {
            f(&mut self.runs[0].style);
            return;
        }
        if range.is_empty() {
            return;
        }
        let a = self.split_runs(range.start);
        let b = self.split_runs(range.end);
        for run in &mut self.runs[a..b] {
            f(&mut run.style);
        }
        self.merge_runs();
    }

    /// Applies `f` to the paragraphs that `range` touches.
    pub fn restyle_paragraphs(&mut self, range: Range<usize>, f: impl Fn(&mut ParagraphStyle)) {
        let (a, b) = (self.paragraph_at(range.start), self.paragraph_at(range.end));
        for p in &mut self.paragraphs[a..=b] {
            f(p);
        }
    }

    /// Index of the paragraph holding byte `i`.
    pub fn paragraph_at(&self, i: usize) -> usize {
        self.text[..i.min(self.text.len())].matches('\n').count()
    }

    /// Byte ranges of the paragraphs (without their line break).
    pub fn paragraph_ranges(&self) -> Vec<Range<usize>> {
        let mut start = 0;
        let mut out = Vec::with_capacity(self.paragraphs.len());
        for line in self.text.split('\n') {
            out.push(start..start + line.len());
            start += line.len() + 1;
        }
        out
    }

    /// Makes the runs cover the text and the paragraphs match its lines,
    /// with valid values (damaged files).
    pub fn repair(&mut self) {
        let mut covered = 0;
        self.runs.retain_mut(|run| {
            run.len = run.len.min(self.text.len() - covered);
            covered += run.len;
            run.style.sanitize();
            run.len > 0
        });
        if covered < self.text.len() || self.runs.is_empty() {
            let style = self
                .runs
                .last()
                .map(|r| r.style.clone())
                .unwrap_or_default();
            self.runs.push(Run {
                len: self.text.len() - covered,
                style,
            });
        }
        // Runs end on character boundaries.
        let mut end = 0;
        for run in &mut self.runs {
            let mut e = (end + run.len).min(self.text.len());
            while !self.text.is_char_boundary(e) {
                e += 1;
            }
            run.len = e.saturating_sub(end);
            end = e.max(end);
        }
        self.merge_runs();
        let n = self.text.matches('\n').count() + 1;
        let last = self.paragraphs.last().copied().unwrap_or_default();
        self.paragraphs.resize(n, last);
        for p in &mut self.paragraphs {
            p.sanitize();
        }
        if let Some((w, h)) = self.frame {
            self.frame = (w.is_finite() && h.is_finite())
                .then(|| (w.clamp(1.0, 100_000.0), h.clamp(1.0, 100_000.0)));
        }
        if !self.transform.iter().all(|v| v.is_finite()) || det(self.transform).abs() < 1e-6 {
            self.transform = IDENTITY;
        }
        if !(self.anchor.0.is_finite() && self.anchor.1.is_finite()) {
            self.anchor = (0.0, 0.0);
        }
    }

    /// Everything that has a size, scaled by `s` (Image Size).
    pub fn scaled(&self, s: f32) -> TextData {
        let mut out = self.clone();
        for run in &mut out.runs {
            let st = &mut run.style;
            st.size = (st.size * s).max(0.1);
            st.leading = st.leading.map(|l| l * s);
            st.baseline_shift *= s;
        }
        for p in &mut out.paragraphs {
            for v in [
                &mut p.indent_left,
                &mut p.indent_right,
                &mut p.indent_first,
                &mut p.space_before,
                &mut p.space_after,
            ] {
                *v *= s;
            }
        }
        out.frame = self
            .frame
            .map(|(w, h)| ((w * s).max(1.0), (h * s).max(1.0)));
        out
    }

    /// Index of the run starting at byte `at`, splitting the one that holds
    /// it.
    fn split_runs(&mut self, at: usize) -> usize {
        let mut start = 0;
        for i in 0..self.runs.len() {
            let len = self.runs[i].len;
            if at == start {
                return i;
            }
            if at < start + len {
                let tail = Run {
                    len: start + len - at,
                    style: self.runs[i].style.clone(),
                };
                self.runs[i].len = at - start;
                self.runs.insert(i + 1, tail);
                return i + 1;
            }
            start += len;
        }
        self.runs.len()
    }

    /// Joins neighbouring runs of the same style and drops empty ones; an
    /// empty text keeps a single empty run.
    fn merge_runs(&mut self) {
        if self.text.is_empty() {
            self.runs.truncate(1);
            self.runs[0].len = 0;
            return;
        }
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..).filter(|r| r.len > 0) {
            match out.last_mut() {
                Some(last) if last.style == run.style => last.len += run.len,
                _ => out.push(run),
            }
        }
        self.runs = out;
    }
}

fn det([a, b, c, d]: [f32; 4]) -> f32 {
    a * d - b * c
}

// ------------------------------------------------------------ text editing

/// Byte index of the character boundary before `i`.
pub fn prev_char(text: &str, i: usize) -> usize {
    text[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

/// Byte index of the character boundary after `i`.
pub fn next_char(text: &str, i: usize) -> usize {
    text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// Start of the word before `i` (Ctrl+Left).
pub fn word_left(text: &str, i: usize) -> usize {
    let mut j = i;
    let mut seen_word = false;
    for (k, c) in text[..i].char_indices().rev() {
        if c == '\n' {
            if j == i {
                j = k;
            }
            break;
        }
        if is_word(c) {
            seen_word = true;
        } else if seen_word {
            break;
        }
        j = k;
    }
    j
}

/// End of the word after `i` (Ctrl+Right), past the spaces that follow.
pub fn word_right(text: &str, i: usize) -> usize {
    let mut chars = text[i..].char_indices().peekable();
    let mut j = i;
    while let Some(&(k, c)) = chars.peek() {
        if !is_word(c) {
            break;
        }
        j = i + k + c.len_utf8();
        chars.next();
    }
    if j == i
        && let Some((k, c)) = chars.next()
    {
        j = i + k + c.len_utf8();
    }
    for (k, c) in chars {
        if c != ' ' {
            break;
        }
        j = i + k + 1;
    }
    j
}

/// The word (or the run of other characters) around byte `i`
/// (double-click).
pub fn word_at(text: &str, i: usize) -> Range<usize> {
    let kind = |c: char| (is_word(c), c.is_whitespace());
    let Some(c) = text[i..]
        .chars()
        .next()
        .filter(|&c| c != '\n')
        .or_else(|| text[..i].chars().next_back())
    else {
        return i..i;
    };
    let k = kind(c);
    let mut start = i;
    for (j, c) in text[..i].char_indices().rev() {
        if kind(c) != k || c == '\n' {
            break;
        }
        start = j;
    }
    let mut end = i;
    for (j, c) in text[i..].char_indices() {
        if kind(c) != k || c == '\n' {
            break;
        }
        end = i + j + c.len_utf8();
    }
    start..end
}

/// Shows `c` as the text shows it (all caps, small caps): the character
/// laid out, and whether it is a faux small capital.
fn displayed(c: char, style: &CharStyle) -> (char, bool) {
    if !(style.all_caps || style.small_caps) {
        return (c, false);
    }
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) if u != c => (u, style.small_caps && !style.all_caps),
        _ => (c, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red() -> CharStyle {
        CharStyle {
            color: [255, 0, 0, 255],
            ..Default::default()
        }
    }

    #[test]
    fn auto_name_uses_first_line() {
        let mut d = TextData::plain("Hello\nworld", CharStyle::default());
        assert_eq!(d.auto_name(), "Hello");
        d.text = "  ".into();
        assert_eq!(d.auto_name(), "Text");
        d.text = "x".repeat(30);
        assert_eq!(d.auto_name().chars().count(), 25);
    }

    #[test]
    fn runs_follow_insertions_deletions_and_restyling() {
        let mut d = TextData::plain("Hello world", CharStyle::default());
        d.restyle(6..11, |s| s.color = [255, 0, 0, 255]);
        assert_eq!(d.runs.len(), 2);
        assert_eq!((d.runs[0].len, d.runs[1].len), (6, 5));
        // Typing after "w" takes its style.
        assert_eq!(d.style_at(7), &red());
        assert_eq!(d.style_at(6), &CharStyle::default(), "before the space");
        d.insert(7, "éé", &red());
        assert_eq!(d.text, "Hello wééorld");
        assert_eq!(d.runs.len(), 2, "merged with its run");
        // Deleting across both runs.
        d.delete(3..9);
        assert_eq!(d.text, "Heléorld");
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), d.text.len());
        // Restyling everything back merges into one run.
        d.restyle(0..d.text.len(), |s| s.color = [0, 0, 0, 255]);
        assert_eq!(d.runs.len(), 1);
        // Emptied, it keeps the style to type with.
        d.restyle(0..3, |s| s.size = 10.0);
        d.delete(0..d.text.len());
        assert_eq!(d.runs.len(), 1);
        assert_eq!(d.runs[0].len, 0);
        assert_eq!(d.runs[0].style.size, 10.0);
        d.insert(0, "a", &d.style_at(0).clone());
        assert_eq!(d.runs[0].style.size, 10.0);
    }

    #[test]
    fn paragraphs_follow_line_breaks() {
        let mut d = TextData::plain("ab", CharStyle::default());
        d.paragraphs[0].align = TextAlign::Center;
        d.insert(1, "\n\n", &CharStyle::default());
        assert_eq!(d.paragraphs.len(), 3);
        assert!(d.paragraphs.iter().all(|p| p.align == TextAlign::Center));
        d.restyle_paragraphs(4..4, |p| p.align = TextAlign::Right);
        assert_eq!(d.paragraphs[2].align, TextAlign::Right);
        assert_eq!(d.paragraph_ranges(), vec![0..1, 2..2, 3..4]);
        // Joining keeps the first one's style.
        d.delete(1..2);
        assert_eq!(d.text, "a\nb");
        assert_eq!(d.paragraphs.len(), 2);
        assert_eq!(d.paragraphs[0].align, TextAlign::Center);
        assert_eq!(d.paragraphs[1].align, TextAlign::Right);
    }

    #[test]
    fn repair_fixes_damaged_runs() {
        let mut d = TextData::plain("éa\nb", CharStyle::default());
        d.runs = vec![Run {
            len: 1,
            style: CharStyle {
                size: f32::NAN,
                ..red()
            },
        }];
        d.paragraphs.clear();
        d.transform = [0.0; 4];
        d.repair();
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), d.text.len());
        let mut end = 0;
        for run in &d.runs {
            end += run.len;
            assert!(d.text.is_char_boundary(end), "runs end on characters");
        }
        assert!(d.runs[0].style.size.is_finite());
        assert_eq!(d.paragraphs.len(), 2);
        assert_eq!(d.transform, IDENTITY);
    }

    #[test]
    fn word_navigation() {
        let t = "Hello, big world\nnext";
        assert_eq!(word_left(t, 16), 11);
        assert_eq!(word_left(t, 11), 7);
        assert_eq!(word_right(t, 0), 5);
        assert_eq!(word_right(t, 7), 11);
        assert_eq!(word_at(t, 8), 7..10);
        assert_eq!(word_at(t, 16), 11..16, "end of the line: the word before");
        assert_eq!(word_left(t, 17), 16, "stops at the line break");
        assert_eq!(prev_char("aé", 3), 1);
        assert_eq!(next_char("éa", 0), 2);
    }

    #[test]
    fn scaling_keeps_proportions() {
        let mut d = TextData::plain("a", CharStyle::default());
        d.runs[0].style.leading = Some(60.0);
        d.paragraphs[0].indent_left = 10.0;
        d.frame = Some((100.0, 50.0));
        let s = d.scaled(2.0);
        assert_eq!(s.runs[0].style.size, 96.0);
        assert_eq!(s.runs[0].style.leading, Some(120.0));
        assert_eq!(s.paragraphs[0].indent_left, 20.0);
        assert_eq!(s.frame, Some((200.0, 100.0)));
    }
}
