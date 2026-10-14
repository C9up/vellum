//! Flowing text into lines, the way pdfkit's `LineWrapper` does.
//!
//! The algorithm is pdfkit's, step for step, because a document ported from
//! pdfkit must break its lines at the same words and its pages at the same
//! lines: words run from one UAX #14 break opportunity to the next and carry
//! their trailing space; a word fits when its width, space included, fits
//! what is left of the line; a word wider than the line is cut; a soft hyphen
//! shows as `-` where the line breaks there; a paragraph's last line is never
//! justified; the first line of a paragraph never sits alone at the bottom of
//! a page.
//!
//! Where it differs, it does more: lines are put in visual order (UAX #9), so
//! Arabic and Hebrew read right; a word too long for the line is cut between
//! graphemes, not inside one.
//!
//! A page break hands control back to the caller ([`Step::NeedPage`]), which
//! adds the page — running whatever is listening for a new page, a header
//! drawn with `text()` included — and resumes. The run waits on a stack, so
//! text drawn while it waits does not disturb it.

use std::ops::Range;

use rustybuzz::Feature;
use unicode_bidi::{BidiInfo, Level};
use unicode_linebreak::{linebreaks, BreakOpportunity};
use unicode_segmentation::UnicodeSegmentation;

use super::fonts::{parse_features, LoadedFont, Shaped};
use super::ops::{Op, PlacedGlyph};
use super::{ComposeError, Composer};

const SOFT_HYPHEN: char = '\u{00AD}';
const HYPHEN: &str = "-";
/// Words remembered with their widths; past this the cache starts over.
const WIDTH_CACHE_LIMIT: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Left for a left-to-right paragraph, right for a right-to-left one.
    #[default]
    Start,
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Baseline {
    /// `y` is the top of the ascender.
    #[default]
    Top,
    Alphabetic,
    Middle,
    SvgMiddle,
    Bottom,
    Mathematical,
    Hanging,
    /// Points from `y` to the baseline, upwards (pdfkit's numeric baseline).
    Offset(f32),
}

/// `text()`'s options, already resolved by the caller: defaults applied,
/// earlier options of a continued paragraph merged in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextOptions {
    /// Wrap width. `None` (or 0) draws each line as given, and moves `x`.
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub align: Align,
    pub line_gap: Option<f32>,
    pub paragraph_gap: f32,
    pub indent: Option<f32>,
    pub indent_all_lines: bool,
    pub columns: u32,
    pub column_gap: f32,
    /// Already resolved: `true` is `…`.
    pub ellipsis: Option<String>,
    pub continued: bool,
    pub character_spacing: Option<f32>,
    pub word_spacing: Option<f32>,
    /// Percent; 100 is none.
    pub horizontal_scaling: f32,
    pub features: Option<Vec<String>>,
    pub baseline: Baseline,
}

/// Whether a run of text finished or needs a page to go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Done,
    NeedPage,
}

/// What a laid-out line is drawn with.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LineInfo {
    text_width: Option<f32>,
    word_count: Option<usize>,
    line_width: Option<f32>,
    align: Align,
    rtl: bool,
}

/// What the run does with each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sink {
    /// Draw it, then move down (or right, unwrapped).
    Draw,
    /// Only move down: measuring.
    Measure,
    /// Move down and keep the widest line: the bounds of the text.
    Bounds,
}

/// pdfkit's `LineWrapper`: the state that lives across the lines of one
/// text, and across the texts of a continued paragraph.
#[derive(Debug, Clone)]
pub(crate) struct Wrapper {
    horizontal_scaling: f32,
    indent: f32,
    indent_all_lines: bool,
    character_spacing: f32,
    word_spacing: f32,
    columns: u32,
    column_gap: f32,
    line_width: f32,
    space_left: f32,
    start_x: f32,
    start_y: f32,
    column: u32,
    ellipsis: Option<String>,
    continued_x: f32,
    indent_applied: bool,
    height: Option<f32>,
    max_y: f32,
    /// Hooks pdfkit registers with `once('line')`, run after the next line.
    after_line: Vec<AfterLine>,
    align_override: Option<Align>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AfterLine {
    /// Take the first-line indent back off.
    Unindent { indent: f32, continued: bool },
    /// With `indentAllLines`, only the continuation offset comes off.
    RemoveContinuation(f32),
    /// End of a paragraph: its gap, and the alignment justify had replaced.
    ParagraphEnd { gap: f32 },
}

/// Where a suspended run picks up once its page exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// Before the first word: the orphan check moved to a new page.
    Start,
    /// After a line was emitted at the bottom of a page.
    AfterLine,
}

/// One break opportunity: the text before it, back to the previous one, is
/// a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Break {
    position: usize,
    required: bool,
}

/// A piece of a word cut because it was wider than the line.
#[derive(Debug, Clone, PartialEq)]
struct Piece {
    /// The rest of the word still to place.
    rest: String,
    width: f32,
    /// The break the whole word ends at.
    end: Break,
    /// The break before this piece; `None` on the first piece of the text.
    last: Option<Break>,
}

/// One `text()` call in progress.
#[derive(Debug, Clone)]
pub(crate) struct TextRun {
    text: String,
    options: TextOptions,
    features: Vec<Feature>,
    wrapper: Wrapper,
    sink: Sink,
    breaks: Vec<Break>,
    next: usize,
    last: Option<Break>,
    pieces: Option<Piece>,
    buffer: String,
    text_width: f32,
    word_count: usize,
    line_count: usize,
    continue_y: f32,
    /// The direction of the paragraph being wrapped.
    rtl: bool,
    /// The text width of the last line out: where a continued text picks up.
    last_text_width: f32,
    /// A section ran out and only a new page lets the run go on.
    needs_page: bool,
    /// The widest line so far, for [`Sink::Bounds`].
    content_width: f32,
    /// Set when suspended.
    resume: Option<Resume>,
    /// State a suspended word callback needs to finish.
    pending: Option<PendingWord>,
    started: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct PendingWord {
    word: String,
    width: f32,
    required: bool,
}

/// What the word callback asks of the loop.
enum Flow {
    Continue,
    Stop,
    Suspend,
}

fn first_strong_is_rtl(text: &str) -> bool {
    text.chars()
        .find_map(|character| match unicode_bidi::bidi_class(character) {
            unicode_bidi::BidiClass::L => Some(false),
            unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL => Some(true),
            _ => None,
        })
        .unwrap_or(false)
}

/// Break opportunities as pdfkit's `linebreak` gives them: the end of the
/// text is never a required break.
fn break_opportunities(text: &str) -> Vec<Break> {
    linebreaks(text)
        .map(|(position, opportunity)| Break {
            position,
            required: opportunity == BreakOpportunity::Mandatory && position < text.len(),
        })
        .collect()
}

/// UTF-16 length, as JavaScript counts a string: what pdfkit multiplies
/// character spacing by.
fn js_length(text: &str) -> f32 {
    text.encode_utf16().count() as f32
}

impl Composer {
    pub fn current_font(&self) -> Result<&LoadedFont, ComposeError> {
        self.font
            .and_then(|index| self.fonts.get(index))
            .ok_or_else(|| ComposeError::new("NO_FONT", "no font is selected: call font() first"))
    }

    /// The advance of `text` at the current font and size, in thousandths
    /// of an em, shaped word by word as pdfkit does unless features are
    /// asked for.
    fn advance(&mut self, text: &str, features: &[Feature]) -> Result<f32, ComposeError> {
        let index = self.font.ok_or_else(|| {
            ComposeError::new("NO_FONT", "no font is selected: call font() first")
        })?;
        if !features.is_empty() {
            return self.current_font()?.width(text, 1000.0, features);
        }
        let mut total = 0.0;
        for chunk in text.split_inclusive([' ', '\t']) {
            if let Some(width) = self.width_cache.get(&(index, chunk.to_string())) {
                total += width;
                continue;
            }
            let width = self.current_font()?.width(chunk, 1000.0, &[])?;
            if self.width_cache.len() >= WIDTH_CACHE_LIMIT {
                self.width_cache.clear();
            }
            self.width_cache.insert((index, chunk.to_string()), width);
            total += width;
        }
        Ok(total)
    }

    /// pdfkit's `widthOfString`.
    pub fn width_of_string(
        &mut self,
        text: &str,
        character_spacing: f32,
        horizontal_scaling: f32,
        features: &[Feature],
    ) -> Result<f32, ComposeError> {
        let font_width = self.advance(text, features)? / 1000.0 * self.font_size;
        Ok((font_width + character_spacing * (js_length(text) - 1.0)) * horizontal_scaling / 100.0)
    }

    pub fn current_line_height(&self, include_gap: bool) -> Result<f32, ComposeError> {
        Ok(self
            .current_font()?
            .line_height(self.font_size, include_gap))
    }

    pub fn move_down(&mut self, lines: f32) -> Result<(), ComposeError> {
        self.y += self.current_line_height(true)? * lines + self.line_gap;
        Ok(())
    }

    pub fn move_up(&mut self, lines: f32) -> Result<(), ComposeError> {
        self.y -= self.current_line_height(true)? * lines + self.line_gap;
        Ok(())
    }

    /// Start drawing `text` at the cursor. [`Step::NeedPage`] asks for a new
    /// page, then [`Self::resume_text`].
    pub fn text(&mut self, text: &str, options: &TextOptions) -> Result<Step, ComposeError> {
        self.page()?;
        let run = self.start_run(text, options, Sink::Draw)?;
        self.drive(run)
    }

    /// Pick up the run that asked for a page, now that it has one.
    pub fn resume_text(&mut self) -> Result<Step, ComposeError> {
        let run = self.suspended.pop().ok_or_else(|| {
            ComposeError::new("NOTHING_TO_RESUME", "no text is waiting for a page")
        })?;
        self.drive(run)
    }

    /// pdfkit's `heightOfString`: the same layout, drawing nothing, never
    /// breaking the page.
    pub fn height_of_string(
        &mut self,
        text: &str,
        options: &TextOptions,
    ) -> Result<f32, ComposeError> {
        let (x, y) = (self.x, self.y);
        let measuring = TextOptions {
            height: Some(f32::INFINITY),
            continued: false,
            ..options.clone()
        };
        let continued = self.wrapper.take();
        let run = self.start_run(text, &measuring, Sink::Measure);
        let result = run.and_then(|run| self.drive(run));
        self.wrapper = continued;
        let height = self.y - y;
        self.x = x;
        self.y = y;
        result.map(|_| height)
    }

    /// pdfkit's `boundsOfString`: `(width, height)` of what `text()` would
    /// draw, before any rotation. Never breaks the page — pdfkit's does,
    /// adding real pages to the document while it measures.
    pub fn bounds_of_string(
        &mut self,
        text: &str,
        options: &TextOptions,
    ) -> Result<(f32, f32), ComposeError> {
        let (x, y) = (self.x, self.y);
        // A height still stops the text where it would; without one, no
        // page break.
        let measuring = TextOptions {
            height: Some(options.height.unwrap_or(f32::INFINITY)),
            continued: false,
            ..options.clone()
        };
        let continued = self.wrapper.take();
        let width = self.measure_bounds(text, &measuring);
        self.wrapper = continued;
        let mut height = self.y - y;
        if let Some(limit) = options.height.filter(|limit| *limit != 0.0) {
            height = height.min(limit);
        }
        self.x = x;
        self.y = y;
        width.map(|width| (width, height))
    }

    fn measure_bounds(&mut self, text: &str, options: &TextOptions) -> Result<f32, ComposeError> {
        let mut run = self.start_run(text, options, Sink::Bounds)?;
        if options.width.is_some_and(|width| width != 0.0) {
            self.wrap(&mut run)?;
        } else {
            self.unwrapped(&mut run)?;
        }
        Ok(run.content_width)
    }

    fn start_run(
        &mut self,
        text: &str,
        options: &TextOptions,
        sink: Sink,
    ) -> Result<TextRun, ComposeError> {
        self.current_font()?;
        let features = parse_features(options.features.as_deref().unwrap_or(&[]))?;
        // pdfkit collapses runs of white space when word spacing is set.
        let text = if options.word_spacing.is_some_and(|spacing| spacing != 0.0) {
            collapse_whitespace(text)
        } else {
            text.to_string()
        };
        let wrapper = match (sink, self.wrapper.take()) {
            (Sink::Draw, Some(wrapper)) => wrapper,
            _ => self.new_wrapper(options)?,
        };
        Ok(TextRun {
            breaks: break_opportunities(&text),
            rtl: first_strong_is_rtl(&text),
            text,
            options: options.clone(),
            features,
            wrapper,
            sink,
            next: 0,
            last: None,
            pieces: None,
            buffer: String::new(),
            text_width: 0.0,
            word_count: 0,
            line_count: 0,
            continue_y: self.y,
            last_text_width: 0.0,
            needs_page: false,
            content_width: 0.0,
            resume: None,
            pending: None,
            started: false,
        })
    }

    fn new_wrapper(&self, options: &TextOptions) -> Result<Wrapper, ComposeError> {
        let horizontal_scaling = options.horizontal_scaling;
        let scale = horizontal_scaling / 100.0;
        let columns = options.columns.max(1);
        let column_gap = options.column_gap * scale;
        let width = options.width.unwrap_or(0.0);
        let line_width = (width * scale - column_gap * (columns as f32 - 1.0)) / columns as f32;
        let max_y = match options.height {
            Some(height) => self.y + height,
            None => self.page()?.max_y(),
        };
        Ok(Wrapper {
            horizontal_scaling,
            indent: options.indent.unwrap_or(0.0) * scale,
            indent_all_lines: options.indent_all_lines,
            character_spacing: options.character_spacing.unwrap_or(0.0) * scale,
            word_spacing: 0.0,
            columns,
            column_gap,
            line_width,
            space_left: line_width,
            start_x: self.x,
            start_y: self.y,
            column: 1,
            ellipsis: options.ellipsis.clone(),
            continued_x: 0.0,
            indent_applied: false,
            height: options.height,
            max_y,
            after_line: Vec::new(),
            align_override: None,
        })
    }

    fn drive(&mut self, mut run: TextRun) -> Result<Step, ComposeError> {
        let wraps = run.options.width.is_some_and(|width| width != 0.0);
        let step = if wraps {
            self.wrap(&mut run)?
        } else {
            self.unwrapped(&mut run)?;
            Step::Done
        };
        match step {
            Step::NeedPage => self.suspended.push(run),
            Step::Done => {
                if run.options.continued && run.sink == Sink::Draw && wraps {
                    self.wrapper = Some(run.wrapper);
                }
            }
        }
        Ok(step)
    }

    /// No width: each line as given, the cursor moving right after it.
    fn unwrapped(&mut self, run: &mut TextRun) -> Result<(), ComposeError> {
        let text = run.text.clone();
        for line in text.split('\n') {
            let info = LineInfo {
                text_width: None,
                word_count: None,
                line_width: None,
                align: run.options.align,
                rtl: first_strong_is_rtl(line),
            };
            match run.sink {
                Sink::Draw => {
                    self.fragment(line, self.x, self.y, run, &info)?;
                    let features = run.features.clone();
                    self.x += self.width_of_string(
                        line,
                        run.options.character_spacing.unwrap_or(0.0),
                        run.options.horizontal_scaling,
                        &features,
                    )?;
                }
                Sink::Measure => self.advance_line(run)?,
                Sink::Bounds => {
                    let features = run.features.clone();
                    let width = self.width_of_string(
                        line,
                        run.options.character_spacing.unwrap_or(0.0),
                        run.options.horizontal_scaling,
                        &features,
                    )?;
                    self.advance_bounds(run)?;
                    run.content_width = run.content_width.max(width);
                }
            }
        }
        Ok(())
    }

    fn line_gap_for(&self, run: &TextRun) -> f32 {
        match run.options.line_gap {
            Some(gap) if gap != 0.0 => gap,
            _ => self.line_gap,
        }
    }

    fn advance_line(&mut self, run: &TextRun) -> Result<(), ComposeError> {
        self.y += self.current_line_height(true)? + self.line_gap_for(run);
        Ok(())
    }

    /// pdfkit's bounds take the line gap with `??`, not `||`: an explicit 0
    /// wins over the document's gap.
    fn advance_bounds(&mut self, run: &TextRun) -> Result<(), ComposeError> {
        let gap = run.options.line_gap.unwrap_or(self.line_gap);
        self.y += self.current_line_height(true)? + gap;
        Ok(())
    }

    /// How wide a wrapped line is drawn: its text width, or for a justified
    /// line its words and the space spread between them. pdfkit adds the
    /// spread on top of a width that keeps the trailing space, and word and
    /// character spacing a second time on top of a width that already carries
    /// them; a justified line then measures wider than the line it fills.
    fn rendered_width(
        &mut self,
        line: &str,
        run: &TextRun,
        info: &LineInfo,
    ) -> Result<f32, ComposeError> {
        let line: String = line
            .chars()
            .filter(|&character| character != '\n')
            .collect();
        if line.is_empty() {
            return Ok(0.0);
        }
        let text_width = info.text_width.unwrap_or(0.0);
        let (Align::Justify, Some(line_width)) = (info.align, info.line_width) else {
            return Ok(text_width);
        };
        let character_spacing = run.options.character_spacing.unwrap_or(0.0);
        let features = run.features.clone();
        let words = line.split_whitespace().count();
        let compact: String = line.split_whitespace().collect();
        let compact_width = self.width_of_string(
            &compact,
            character_spacing,
            run.options.horizontal_scaling,
            &features,
        )?;
        let space = self.width_of_string(" ", 0.0, 100.0, &[])? + character_spacing;
        let spread = ((line_width - compact_width) / (words.max(2) - 1) as f32 - space).max(0.0);
        Ok(compact_width + (words.saturating_sub(1)) as f32 * (spread + space))
    }

    fn word_width(&mut self, run: &TextRun, word: &str) -> Result<f32, ComposeError> {
        let features = run.features.clone();
        Ok(self.width_of_string(
            word,
            run.wrapper.character_spacing,
            run.wrapper.horizontal_scaling,
            &features,
        )? + run.wrapper.character_spacing
            + run.wrapper.word_spacing)
    }

    fn can_fit(&mut self, run: &TextRun, word: &str, width: f32) -> Result<bool, ComposeError> {
        if !word.ends_with(SOFT_HYPHEN) {
            return Ok(width <= run.wrapper.space_left);
        }
        Ok(width + self.word_width(run, HYPHEN)? <= run.wrapper.space_left)
    }

    /// pdfkit's `wrap`, resumable.
    fn wrap(&mut self, run: &mut TextRun) -> Result<Step, ComposeError> {
        if !run.started {
            run.started = true;
            let scale_options = &run.options;
            let wrapper = &mut run.wrapper;
            wrapper.horizontal_scaling = scale_options.horizontal_scaling;
            let scale = wrapper.horizontal_scaling / 100.0;
            if let Some(indent) = scale_options.indent {
                wrapper.indent = indent * scale;
            }
            if let Some(spacing) = scale_options.character_spacing {
                wrapper.character_spacing = spacing * scale;
            }
            if let Some(spacing) = scale_options.word_spacing {
                wrapper.word_spacing = spacing * scale;
            }
            if let Some(ellipsis) = &scale_options.ellipsis {
                wrapper.ellipsis = Some(ellipsis.clone());
            }
            // Never begin on the page's edge, and never leave a first line
            // alone at its bottom.
            let next_y = self.y + self.current_line_height(true)?;
            // pdfkit carries on even when a maximum height is already
            // reached: the first line out then stops the run.
            if (self.y > run.wrapper.max_y || next_y > run.wrapper.max_y)
                && self.next_section(run)?.is_none()
            {
                run.resume = Some(Resume::Start);
                return Ok(Step::NeedPage);
            }
            run.continue_y = self.y;
        } else if let Some(resume) = run.resume.take() {
            self.after_page_break(run);
            match resume {
                Resume::Start => run.continue_y = self.y,
                Resume::AfterLine => {
                    if let Some(pending) = run.pending.take() {
                        self.finish_word(run, &pending.word, pending.width, pending.required);
                    }
                }
            }
        }

        loop {
            match self.next_word(run)? {
                None => break,
                Some(Flow::Continue) => {}
                Some(Flow::Stop) => {
                    run.word_count = 0;
                    run.buffer.clear();
                    break;
                }
                Some(Flow::Suspend) => {
                    run.resume = Some(Resume::AfterLine);
                    return Ok(Step::NeedPage);
                }
            }
        }

        if run.word_count > 0 {
            self.last_line(run);
            self.emit_line(run)?;
        }
        self.end_wrap(run);
        Ok(Step::Done)
    }

    fn end_wrap(&mut self, run: &mut TextRun) {
        if run.options.continued {
            if run.line_count > 1 {
                run.wrapper.continued_x = 0.0;
            }
            run.wrapper.continued_x += run.last_text_width;
            self.y = run.continue_y;
        } else {
            self.x = run.wrapper.start_x;
        }
    }

    /// The next word (or piece of one) through the word callback; `None`
    /// once the text is used up.
    fn next_word(&mut self, run: &mut TextRun) -> Result<Option<Flow>, ComposeError> {
        if let Some(piece) = run.pieces.take() {
            return self.next_piece(run, piece).map(Some);
        }
        let Some(&end) = run.breaks.get(run.next) else {
            return Ok(None);
        };
        run.next += 1;
        let start = run.last.map_or(0, |last| last.position);
        let word = run.text[start..end.position].to_string();
        let width = self.word_width(run, &word)?;
        if width > run.wrapper.line_width + run.wrapper.continued_x {
            let piece = Piece {
                rest: word,
                width,
                end,
                last: run.last,
            };
            return self.next_piece(run, piece).map(Some);
        }
        let last = run.last;
        let flow = self.on_word(run, &word, width, end, last)?;
        if !matches!(flow, Flow::Stop) {
            run.last = Some(end);
        }
        Ok(Some(flow))
    }

    /// pdfkit's cutting of a word wider than the line, a piece at a time.
    fn next_piece(&mut self, run: &mut TextRun, piece: Piece) -> Result<Flow, ComposeError> {
        let Piece {
            rest,
            mut width,
            end,
            last,
        } = piece;
        let graphemes: Vec<&str> = rest.graphemes(true).collect();
        let total = graphemes.len();
        let prefix = |count: usize| -> String { graphemes[..count].concat() };
        let mut count;
        let mut might_grow = false;
        if width > run.wrapper.space_left {
            count = ((run.wrapper.space_left / (width / total as f32)).ceil() as usize).min(total);
            width = self.word_width(run, &prefix(count))?;
            might_grow = width <= run.wrapper.space_left && count < total;
        } else {
            count = total;
        }
        let mut must_shrink = width > run.wrapper.space_left && count > 0;
        while must_shrink || might_grow {
            if must_shrink {
                count -= 1;
                width = self.word_width(run, &prefix(count))?;
                must_shrink = width > run.wrapper.space_left && count > 0;
            } else {
                count += 1;
                width = self.word_width(run, &prefix(count))?;
                must_shrink = width > run.wrapper.space_left && count > 0;
                might_grow = width <= run.wrapper.space_left && count < total;
            }
        }
        if count == 0 && run.wrapper.space_left == run.wrapper.line_width {
            count = 1;
            width = self.word_width(run, &prefix(count))?;
        }
        let head = prefix(count);
        let tail: String = graphemes[count..].concat();
        let fake = Break {
            position: end.position,
            required: end.required || count < total,
        };
        let flow = self.on_word(run, &head, width, fake, last)?;
        if !tail.is_empty() && !matches!(flow, Flow::Stop) {
            let tail_width = self.word_width(run, &tail)?;
            run.pieces = Some(Piece {
                rest: tail,
                width: tail_width,
                end,
                last: Some(Break {
                    position: end.position,
                    required: false,
                }),
            });
        } else if !matches!(flow, Flow::Stop) {
            run.last = Some(end);
        }
        Ok(flow)
    }

    /// The body of pdfkit's `eachWord` callback.
    fn on_word(
        &mut self,
        run: &mut TextRun,
        word: &str,
        width: f32,
        end: Break,
        last: Option<Break>,
    ) -> Result<Flow, ComposeError> {
        if last.is_none_or(|last| last.required) {
            self.first_line(run);
            run.wrapper.space_left = run.wrapper.line_width;
        }

        let fits = self.can_fit(run, word, width)?;
        if fits {
            run.buffer.push_str(word);
            run.text_width += width;
            run.word_count += 1;
        }
        if !(end.required || !fits) {
            run.wrapper.space_left -= width;
            return Ok(Flow::Continue);
        }

        let line_height = self.current_line_height(true)?;
        if let Some(ellipsis) = run.wrapper.ellipsis.clone() {
            if run.wrapper.height.is_some()
                && self.y + line_height * 2.0 > run.wrapper.max_y
                && run.wrapper.column >= run.wrapper.columns
            {
                self.apply_ellipsis(run, &ellipsis)?;
            }
        }

        if end.required {
            if width > run.wrapper.space_left {
                self.emit_line(run)?;
                run.buffer = word.to_string();
                run.text_width = width;
                run.word_count = 1;
            }
            self.last_line(run);
        }

        if run.buffer.ends_with(SOFT_HYPHEN) {
            run.buffer.pop();
            run.buffer.push_str(HYPHEN);
            let hyphen = self.word_width(run, HYPHEN)?;
            run.wrapper.space_left -= hyphen;
        }

        self.emit_line(run)?;

        if self.y + line_height > run.wrapper.max_y {
            match self.next_section(run)? {
                Some(false) => return Ok(Flow::Stop),
                Some(true) => {}
                None => {
                    run.pending = Some(PendingWord {
                        word: word.to_string(),
                        width,
                        required: end.required,
                    });
                    return Ok(Flow::Suspend);
                }
            }
        }
        self.finish_word(run, word, width, end.required);
        Ok(Flow::Continue)
    }

    /// The end of the callback once the line is out: what the next line
    /// starts with.
    fn finish_word(&mut self, run: &mut TextRun, word: &str, width: f32, required: bool) {
        if required {
            run.wrapper.space_left = run.wrapper.line_width;
            run.buffer.clear();
            run.text_width = 0.0;
            run.word_count = 0;
        } else {
            run.wrapper.space_left = run.wrapper.line_width - width;
            run.buffer = word.to_string();
            run.text_width = width;
            run.word_count = 1;
        }
    }

    fn apply_ellipsis(&mut self, run: &mut TextRun, ellipsis: &str) -> Result<(), ComposeError> {
        let trimmed = run.buffer.trim_end().to_string();
        run.buffer = trimmed;
        let mut width = self.word_width(run, &format!("{}{ellipsis}", run.buffer))?;
        while !run.buffer.is_empty() && width > run.wrapper.line_width {
            run.buffer.pop();
            let trimmed = run.buffer.trim_end().to_string();
            run.buffer = trimmed;
            width = self.word_width(run, &format!("{}{ellipsis}", run.buffer))?;
        }
        if width <= run.wrapper.line_width {
            run.buffer.push_str(ellipsis);
        }
        let buffer = run.buffer.clone();
        run.text_width = self.word_width(run, &buffer)?;
        Ok(())
    }

    /// pdfkit's `firstLine` listener: the paragraph indent, or the offset a
    /// continued text carries on from.
    fn first_line(&mut self, run: &mut TextRun) {
        // The direction of the paragraph that starts here.
        let start = run.last.map_or(0, |last| last.position);
        let paragraph_end = run.text[start..]
            .find('\n')
            .map_or(run.text.len(), |offset| start + offset);
        run.rtl = first_strong_is_rtl(&run.text[start..paragraph_end]);

        let wrapper = &mut run.wrapper;
        let continuation = wrapper.continued_x;
        let indent = if continuation != 0.0 {
            continuation
        } else {
            wrapper.indent
        };
        if continuation != 0.0 || !wrapper.indent_applied {
            self.x += indent;
            wrapper.line_width -= indent;
            wrapper.indent_applied = true;
        }
        if run.options.indent_all_lines {
            if continuation != 0.0 {
                wrapper
                    .after_line
                    .push(AfterLine::RemoveContinuation(continuation));
            }
            return;
        }
        wrapper.after_line.push(AfterLine::Unindent {
            indent,
            continued: run.options.continued,
        });
    }

    /// pdfkit's `lastLine` listener.
    fn last_line(&mut self, run: &mut TextRun) {
        if run.options.align == Align::Justify {
            run.wrapper.align_override = Some(Align::Start);
        }
        run.wrapper.after_line.push(AfterLine::ParagraphEnd {
            gap: run.options.paragraph_gap,
        });
    }

    fn emit_line(&mut self, run: &mut TextRun) -> Result<(), ComposeError> {
        let text_width = run.text_width + run.wrapper.word_spacing * (run.word_count as f32 - 1.0);
        run.last_text_width = text_width;
        let info = LineInfo {
            text_width: Some(text_width),
            word_count: Some(run.word_count),
            line_width: Some(run.wrapper.line_width),
            align: run.wrapper.align_override.unwrap_or(run.options.align),
            rtl: run.rtl,
        };
        run.continue_y = self.y;
        let line = run.buffer.clone();
        match run.sink {
            Sink::Draw => {
                self.fragment(&line, self.x, self.y, run, &info)?;
                self.advance_line(run)?;
            }
            Sink::Measure => self.advance_line(run)?,
            Sink::Bounds => {
                self.advance_bounds(run)?;
                let width = self.rendered_width(&line, run, &info)?;
                run.content_width = run.content_width.max(width);
            }
        }
        run.line_count += 1;
        for hook in std::mem::take(&mut run.wrapper.after_line) {
            match hook {
                AfterLine::Unindent { indent, continued } => {
                    self.x -= indent;
                    run.wrapper.line_width += indent;
                    run.wrapper.indent_applied = false;
                    if continued && run.wrapper.continued_x == 0.0 {
                        run.wrapper.continued_x = run.wrapper.indent;
                    }
                    if !continued {
                        run.wrapper.continued_x = 0.0;
                    }
                }
                AfterLine::RemoveContinuation(continuation) => {
                    self.x -= continuation;
                    run.wrapper.line_width += continuation;
                }
                AfterLine::ParagraphEnd { gap } => {
                    self.y += gap;
                    run.wrapper.align_override = None;
                }
            }
        }
        Ok(())
    }

    /// pdfkit's `nextSection`: the next column, or the next page.
    /// `Some(false)` stops (a maximum height was reached), `None` needs a
    /// page first.
    fn next_section(&mut self, run: &mut TextRun) -> Result<Option<bool>, ComposeError> {
        run.wrapper.column += 1;
        if run.wrapper.column > run.wrapper.columns {
            if run.wrapper.height.is_some() {
                return Ok(Some(false));
            }
            run.needs_page = true;
            return Ok(None);
        }
        self.x += run.wrapper.line_width + run.wrapper.column_gap;
        self.y = run.wrapper.start_y;
        Ok(Some(true))
    }

    /// What pdfkit does once `continueOnNewPage` has added the page.
    fn after_page_break(&mut self, run: &mut TextRun) {
        run.needs_page = false;
        let Ok(page) = self.page() else {
            return;
        };
        let (top, max_y) = (page.setup.margins.top, page.max_y());
        let wrapper = &mut run.wrapper;
        wrapper.column = 1;
        wrapper.start_y = top;
        wrapper.max_y = max_y;
        if wrapper.indent_all_lines {
            if wrapper.indent_applied {
                self.x += if wrapper.continued_x != 0.0 {
                    wrapper.continued_x
                } else {
                    wrapper.indent
                };
            }
        } else {
            self.x = wrapper.start_x;
        }
    }

    /// pdfkit's `_fragment`: one line, aligned, shaped, in visual order.
    fn fragment(
        &mut self,
        text: &str,
        mut x: f32,
        y: f32,
        run: &TextRun,
        info: &LineInfo,
    ) -> Result<(), ComposeError> {
        let text: String = text
            .chars()
            .filter(|&character| character != '\n')
            .collect();
        if text.is_empty() {
            return Ok(());
        }
        let options = &run.options;
        let character_spacing = options.character_spacing.unwrap_or(0.0);
        let mut word_spacing = options.word_spacing.unwrap_or(0.0);
        let scaling = options.horizontal_scaling;
        let features = run.features.clone();
        let align = match info.align {
            Align::Start if info.rtl => Align::Right,
            Align::Start => Align::Left,
            other => other,
        };
        if let (Some(line_width), true) = (info.line_width, options.width.is_some()) {
            match align {
                Align::Right => {
                    let width = self.width_of_string(
                        text.trim_end(),
                        character_spacing,
                        scaling,
                        &features,
                    )?;
                    x += line_width - width;
                }
                Align::Center => {
                    x += line_width / 2.0 - info.text_width.unwrap_or(0.0) / 2.0;
                }
                Align::Justify => {
                    let words = text.split_whitespace().count();
                    let compact: String = text.split_whitespace().collect();
                    let text_width =
                        self.width_of_string(&compact, character_spacing, scaling, &features)?;
                    let space = self.width_of_string(" ", 0.0, 100.0, &[])? + character_spacing;
                    word_spacing =
                        ((line_width - text_width) / (words.max(2) - 1) as f32 - space).max(0.0);
                }
                Align::Left | Align::Start => {}
            }
        }

        let font = self.current_font()?;
        let size = self.font_size;
        let vertical = font.vertical;
        let dy = match options.baseline {
            Baseline::Offset(offset) => -offset,
            named => {
                let units = match named {
                    Baseline::SvgMiddle => 0.5 * vertical.x_height,
                    Baseline::Middle => 0.5 * (vertical.descender + vertical.ascender),
                    Baseline::Bottom => vertical.descender,
                    Baseline::Alphabetic => 0.0,
                    Baseline::Mathematical => 0.5 * vertical.ascender,
                    Baseline::Hanging => 0.8 * vertical.ascender,
                    Baseline::Top | Baseline::Offset(_) => vertical.ascender,
                };
                units / 1000.0 * size
            }
        };

        let glyph_runs =
            self.shape_line(&text, info.rtl, &features, word_spacing, character_spacing)?;
        let font_index = self
            .font
            .ok_or_else(|| ComposeError::new("NO_FONT", "no font is selected"))?;
        let colour = self.fill;
        let mut pen = x;
        let mut ops = Vec::with_capacity(glyph_runs.len());
        for glyphs in glyph_runs {
            let advance: f32 =
                glyphs.iter().map(|glyph| glyph.advance).sum::<f32>() / 1000.0 * size;
            ops.push(Op::Glyphs {
                font: font_index,
                size,
                origin: (pen, y + dy),
                colour,
                text: text.clone(),
                glyphs,
            });
            pen += advance;
        }
        self.page_mut()?.ops.extend(ops);
        Ok(())
    }

    /// The glyphs of one line, one list per visual run, with word and
    /// character spacing folded into the advances (krilla writes neither
    /// `Tw` nor `Tc`; pdfkit folds word spacing the same way).
    fn shape_line(
        &self,
        text: &str,
        paragraph_rtl: bool,
        features: &[Feature],
        word_spacing: f32,
        character_spacing: f32,
    ) -> Result<Vec<Vec<PlacedGlyph>>, ComposeError> {
        let font = self.current_font()?;
        let size = self.font_size;
        let to_units = 1000.0 / size;
        let level = if paragraph_rtl {
            Level::rtl()
        } else {
            Level::ltr()
        };
        let bidi = BidiInfo::new(text, Some(level));
        let Some(paragraph) = bidi.paragraphs.first() else {
            return Ok(Vec::new());
        };
        let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
        let space = if word_spacing != 0.0 {
            font.width(" ", size, &[])? + character_spacing
        } else {
            0.0
        };

        let mut out = Vec::with_capacity(runs.len());
        for range in runs {
            let rtl = levels.get(range.start).is_some_and(|level| level.is_rtl());
            let mut glyphs = Vec::new();
            for (chunk, offset) in chunks(
                text,
                range.clone(),
                features.is_empty(),
                word_spacing != 0.0,
                rtl,
            ) {
                let shaped = font.shape(chunk, features, rtl)?;
                let mut placed = place(&shaped, chunk, offset);
                if let Some(last) = placed.last_mut() {
                    if word_spacing != 0.0 {
                        last.advance += (word_spacing + space) * to_units;
                    }
                }
                glyphs.extend(placed);
            }
            if character_spacing != 0.0 {
                for glyph in &mut glyphs {
                    glyph.advance += character_spacing * to_units;
                }
            }
            if !glyphs.is_empty() {
                out.push(glyphs);
            }
        }
        Ok(out)
    }
}

/// The pieces a run of a line is shaped in, in visual order, with their
/// byte offset in the line. pdfkit shapes word by word (a word carrying its
/// trailing space), and word by word without the spaces when word spacing
/// is set; with OpenType features it shapes the whole run.
fn chunks(
    text: &str,
    range: Range<usize>,
    split: bool,
    words_only: bool,
    rtl: bool,
) -> Vec<(&str, usize)> {
    let run = &text[range.clone()];
    let mut pieces: Vec<(&str, usize)> = if words_only {
        let mut out = Vec::new();
        let mut offset = 0;
        for word in run.split_inclusive(char::is_whitespace) {
            let trimmed = word.trim_end();
            if !trimmed.is_empty() {
                out.push((trimmed, range.start + offset));
            }
            offset += word.len();
        }
        out
    } else if split {
        let mut out = Vec::new();
        let mut offset = 0;
        for chunk in run.split_inclusive([' ', '\t']) {
            out.push((chunk, range.start + offset));
            offset += chunk.len();
        }
        out
    } else {
        vec![(run, range.start)]
    };
    if rtl {
        pieces.reverse();
    }
    pieces
}

/// Shaped glyphs with the bytes of the line each one stands for.
fn place(shaped: &[Shaped], chunk: &str, offset: usize) -> Vec<PlacedGlyph> {
    let mut starts: Vec<usize> = shaped.iter().map(|glyph| glyph.cluster).collect();
    starts.sort_unstable();
    starts.dedup();
    shaped
        .iter()
        .map(|glyph| {
            let end = starts
                .iter()
                .find(|&&start| start > glyph.cluster)
                .copied()
                .unwrap_or(chunk.len());
            PlacedGlyph {
                glyph: glyph.glyph,
                text_start: offset + glyph.cluster,
                text_end: offset + end,
                advance: glyph.advance,
                x_offset: glyph.x_offset,
                y_offset: glyph.y_offset,
            }
        })
        .collect()
}

/// Runs of white space become one space, as pdfkit's `/\s{2,}/g` does.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0;
    for character in text.chars() {
        if character.is_whitespace() {
            run += 1;
            if run == 1 {
                out.push(character);
            } else if run == 2 {
                out.pop();
                out.push(' ');
            }
        } else {
            run = 0;
            out.push(character);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::fonts::tests::liberation_sans;
    use crate::compose::{DocumentSettings, PageSetup, Sides};

    const A4: PageSetup = PageSetup {
        width: 595.28,
        height: 841.89,
        margins: Sides {
            top: 72.0,
            right: 72.0,
            bottom: 72.0,
            left: 72.0,
        },
        user_unit: 1.0,
    };

    const FR: &str = "Les présentes conditions générales régissent l'ensemble des prestations fournies par l'entreprise à ses clients, qu'il s'agisse de travaux de rénovation, d'installations neuves ou d'interventions de dépannage. Toute commande implique l'acceptation sans réserve des conditions ci-après, à l'exclusion de tout autre document. Les devis sont valables trente jours à compter de leur date d'émission ; passé ce délai, les prix peuvent être révisés en fonction de l'évolution du coût des matériaux et de la main-d'œuvre.";

    const FIXTURE: &str = include_str!("../../tests/fixtures/pdfkit-text.tsv");
    const ARABIC: &[u8] = include_bytes!("../../tests/fixtures/NotoSansArabic-Subset.ttf");

    #[derive(Debug, Clone, PartialEq)]
    struct Fragment {
        page: usize,
        text: String,
        x: f32,
        y: f32,
    }

    struct Expected {
        fragments: Vec<Fragment>,
        end: (f32, f32, usize),
        result: Option<f32>,
    }

    fn expected(name: &str) -> Expected {
        let mut fragments = Vec::new();
        let mut end = None;
        let mut result = None;
        for line in FIXTURE.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<&str> = line.splitn(6, '\t').collect();
            if fields[0] != name {
                continue;
            }
            let number = |index: usize| fields[index].parse::<f32>().unwrap();
            if fields[1] == "end" {
                end = Some((number(2), number(3), fields[4].parse().unwrap()));
                result = fields.get(5).map(|value| value.parse().unwrap());
            } else {
                let text = line.splitn(5, '\t').nth(4).unwrap().to_string();
                fragments.push(Fragment {
                    page: fields[1].parse().unwrap(),
                    text,
                    x: number(2),
                    y: number(3),
                });
            }
        }
        Expected {
            fragments,
            end: end.unwrap_or_else(|| panic!("no scenario {name}")),
            result,
        }
    }

    fn bold() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fonts/LiberationSans-Bold.ttf"
        ))
        .unwrap()
    }

    /// A document as pdfkit starts one: A4, a first page, a font.
    fn document() -> Composer {
        let mut document = Composer::new(DocumentSettings {
            buffer_pages: true,
            ..DocumentSettings::default()
        })
        .unwrap();
        document.add_page(A4).unwrap();
        let sans = document.load_font(liberation_sans(), None, None).unwrap();
        document.set_font(sans).unwrap();
        document
    }

    /// What the TypeScript side does around a native `text()`: place the
    /// cursor, default the width, and add pages until the run is done.
    fn text(document: &mut Composer, text: &str, at: Option<(f32, f32)>, mut options: TextOptions) {
        if let Some((x, y)) = at {
            document.x = x;
            document.y = y;
        }
        if options.width.is_none() {
            let page = document.page().unwrap();
            options.width =
                Some((page.setup.width - document.x - page.setup.margins.right).max(0.0));
        }
        let mut step = document.text(text, &options).unwrap();
        while step == Step::NeedPage {
            document.add_page(A4).unwrap();
            step = document.resume_text().unwrap();
        }
    }

    fn unwrapped(document: &mut Composer, text: &str, x: f32, y: f32) {
        document.x = x;
        document.y = y;
        let step = document.text(text, &defaults()).unwrap();
        assert_eq!(step, Step::Done);
    }

    fn defaults() -> TextOptions {
        TextOptions {
            horizontal_scaling: 100.0,
            column_gap: 18.0,
            ..TextOptions::default()
        }
    }

    /// Every line drawn, with the top of its line box, as pdfkit reports it.
    fn fragments(document: &Composer) -> Vec<Fragment> {
        let mut out = Vec::new();
        for (page, contents) in document.pages.iter().enumerate() {
            for op in &contents.ops {
                let Op::Glyphs {
                    font,
                    size,
                    origin,
                    text,
                    ..
                } = op;
                let ascender = document.fonts[*font].vertical.ascender;
                out.push(Fragment {
                    page,
                    text: text.clone(),
                    x: origin.0,
                    y: origin.1 - ascender / 1000.0 * size,
                });
            }
        }
        out
    }

    fn assert_layout(name: &str, document: &Composer) {
        let expected = expected(name);
        let ours = fragments(document);
        let texts: Vec<&str> = ours.iter().map(|fragment| fragment.text.as_str()).collect();
        let theirs: Vec<&str> = expected
            .fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect();
        assert_eq!(texts, theirs, "{name}: the lines differ from pdfkit's");
        for (ours, theirs) in ours.iter().zip(&expected.fragments) {
            assert_eq!(
                ours.page, theirs.page,
                "{name}: {:?} is on another page",
                ours.text
            );
            assert!(
                (ours.x - theirs.x).abs() < 0.01 && (ours.y - theirs.y).abs() < 0.01,
                "{name}: {:?} at ({}, {}), pdfkit puts it at ({}, {})",
                ours.text,
                ours.x,
                ours.y,
                theirs.x,
                theirs.y
            );
        }
        let (x, y, pages) = expected.end;
        assert!(
            (document.x - x).abs() < 0.01 && (document.y - y).abs() < 0.01,
            "{name}: the cursor ends at ({}, {}), pdfkit's at ({x}, {y})",
            document.x,
            document.y
        );
        assert_eq!(document.page_count(), pages, "{name}: page count");
    }

    #[test]
    fn a_paragraph_breaks_where_pdfkit_breaks_it() {
        let mut document = document();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            FR,
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(300.0),
                ..defaults()
            },
        );
        assert_layout("paragraph", &document);
    }

    #[test]
    fn text_flows_onto_the_next_page() {
        let mut document = document();
        for _ in 0..12 {
            text(&mut document, FR, None, defaults());
        }
        assert_layout("pages", &document);
    }

    #[test]
    fn right_and_centre_alignment() {
        for (name, align) in [
            ("align_right", Align::Right),
            ("align_center", Align::Center),
        ] {
            let mut document = document();
            document.set_font_size(11.0).unwrap();
            text(
                &mut document,
                FR,
                Some((72.0, 72.0)),
                TextOptions {
                    width: Some(300.0),
                    align,
                    ..defaults()
                },
            );
            assert_layout(name, &document);
        }
    }

    #[test]
    fn helvetica_lines_sit_where_pdfkit_puts_them() {
        let mut document = document();
        let helvetica = document
            .load_font(liberation_sans(), None, Some("Helvetica"))
            .unwrap();
        document.set_font(helvetica).unwrap();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            FR,
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(300.0),
                ..defaults()
            },
        );
        assert_layout("helvetica", &document);
    }

    #[test]
    fn indent_paragraph_gap_and_line_gap() {
        let mut document = document();
        document.set_font_size(10.0).unwrap();
        document.line_gap = 2.0;
        text(
            &mut document,
            &format!("{FR}\n{FR}"),
            Some((72.0, 100.0)),
            TextOptions {
                width: Some(250.0),
                indent: Some(20.0),
                paragraph_gap: 8.0,
                ..defaults()
            },
        );
        assert_layout("paragraphs", &document);
    }

    #[test]
    fn columns_within_a_height() {
        let mut document = document();
        document.set_font_size(9.0).unwrap();
        text(
            &mut document,
            &format!("{FR} {FR}"),
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(400.0),
                columns: 2,
                column_gap: 20.0,
                height: Some(120.0),
                ..defaults()
            },
        );
        assert_layout("columns", &document);
    }

    #[test]
    fn an_ellipsis_ends_text_cut_by_its_height() {
        let mut document = document();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            FR,
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(200.0),
                height: Some(60.0),
                ellipsis: Some("…".into()),
                ..defaults()
            },
        );
        assert_layout("ellipsis", &document);
    }

    #[test]
    fn a_bold_word_inside_a_sentence() {
        let mut document = document();
        let sans = document.font.unwrap();
        let bold = document.load_font(bold(), None, None).unwrap();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            "Montant total de la prestation : ",
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(200.0),
                continued: true,
                ..defaults()
            },
        );
        document.set_font(bold).unwrap();
        // The options a continued text inherits, as the caller merges them.
        text(
            &mut document,
            "1 200,00 CHF",
            None,
            TextOptions {
                width: Some(200.0),
                continued: true,
                ..defaults()
            },
        );
        document.set_font(sans).unwrap();
        text(
            &mut document,
            " payable à trente jours net, sans escompte.",
            None,
            TextOptions {
                width: Some(200.0),
                ..defaults()
            },
        );
        assert_layout("continued", &document);
    }

    #[test]
    fn height_of_string_is_what_text_would_take() {
        let mut document = document();
        document.set_font_size(11.0).unwrap();
        document.x = 72.0;
        document.y = 72.0;
        let options = TextOptions {
            width: Some(300.0),
            ..defaults()
        };
        let height = document.height_of_string(FR, &options).unwrap();
        let pdfkit = expected("height_of_string").result.unwrap();
        assert!(
            (height - pdfkit).abs() < 0.01,
            "{height} against pdfkit's {pdfkit}"
        );
        assert_eq!(
            (document.x, document.y),
            (72.0, 72.0),
            "measuring leaves the cursor"
        );
        assert!(document.pages[0].ops.is_empty(), "and draws nothing");

        text(&mut document, FR, None, options);
        assert!((document.y - 72.0 - height).abs() < 1e-3);
    }

    #[test]
    fn a_soft_hyphen_and_a_word_wider_than_the_line() {
        let mut document = document();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            "Les installa\u{00AD}tions électriques sont conformes",
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(90.0),
                ..defaults()
            },
        );
        assert_layout("soft_hyphen", &document);

        let mut document = self::document();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            "Référence ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEFGHIJ fin",
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(120.0),
                ..defaults()
            },
        );
        assert_layout("long_word", &document);
    }

    #[test]
    fn a_first_line_is_never_left_alone_at_the_bottom() {
        let mut document = document();
        text(
            &mut document,
            FR,
            Some((72.0, 760.0)),
            TextOptions {
                width: Some(300.0),
                ..defaults()
            },
        );
        assert_layout("orphan", &document);
    }

    #[test]
    fn without_a_width_lines_are_drawn_as_given() {
        let mut document = document();
        unwrapped(&mut document, "Première\nSeconde", 100.0, 100.0);
        assert_layout("unwrapped", &document);
    }

    #[test]
    fn move_down_by_lines() {
        let mut document = document();
        text(&mut document, "A", Some((72.0, 72.0)), defaults());
        document.move_down(2.0).unwrap();
        text(&mut document, "B", None, defaults());
        assert_layout("move_down", &document);
    }

    #[test]
    fn the_text_reads_back() {
        let mut document = document();
        text(
            &mut document,
            FR,
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(300.0),
                ..defaults()
            },
        );
        let bytes = document.end().unwrap();
        let read = crate::extract_text(&bytes, 0).unwrap();
        let words = |text: &str| {
            text.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(words(&read), words(FR));
    }

    #[test]
    fn a_right_to_left_paragraph_is_in_visual_order_and_aligned_right() {
        let mut document = document();
        let font = document.load_font(ARABIC.to_vec(), None, None).unwrap();
        document.set_font(font).unwrap();
        let line = "مرحبا بالعالم";
        text(
            &mut document,
            line,
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(300.0),
                ..defaults()
            },
        );
        let Some(Op::Glyphs { origin, glyphs, .. }) = document.pages[0].ops.first() else {
            panic!("the line is drawn");
        };
        let width: f32 = glyphs.iter().map(|glyph| glyph.advance).sum::<f32>() / 1000.0 * 12.0;
        assert!(
            (origin.0 + width - 372.0).abs() < 0.5,
            "the line ends at the right edge: starts at {} and is {width} wide",
            origin.0
        );
        // Visual order: the first glyph drawn is the last letter written.
        assert!(glyphs[0].text_start > glyphs[glyphs.len() - 1].text_start);

        // And it reads back in the order it was written.
        let bytes = document.end().unwrap();
        let read = crate::extract_text(&bytes, 0).unwrap();
        assert_eq!(
            read.split_whitespace().collect::<Vec<_>>(),
            ["مرحبا", "بالعالم"]
        );
    }

    #[test]
    fn justified_lines_fill_the_width_but_the_last() {
        let mut document = document();
        document.set_font_size(11.0).unwrap();
        text(
            &mut document,
            FR,
            Some((72.0, 72.0)),
            TextOptions {
                width: Some(300.0),
                align: Align::Justify,
                ..defaults()
            },
        );
        let widths: Vec<f32> = document.pages[0]
            .ops
            .iter()
            .map(|op| {
                let Op::Glyphs { glyphs, size, .. } = op;
                // The last glyph's advance carries the word spacing past the
                // line's end; what is drawn stops before it.
                let drawn: f32 = glyphs[..glyphs.len() - 1]
                    .iter()
                    .map(|glyph| glyph.advance)
                    .sum();
                (drawn + 600.0) / 1000.0 * size
            })
            .collect();
        assert_eq!(widths.len(), 9);
        let (body, last) = widths.split_at(8);
        for width in body {
            assert!(
                (width - 300.0).abs() < 8.0,
                "a justified line spans the width: {width}"
            );
        }
        assert!(
            last[0] < 250.0,
            "the last line is not stretched: {}",
            last[0]
        );
    }
}
