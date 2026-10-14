//! Authoring a document from nothing: pages, a cursor, text, and the PDF
//! written at the end.
//!
//! The shape follows pdfkit's: a current page, a cursor (`x`, `y`) that
//! drawing moves, and measurements that answer at once, because a caller
//! lays out the next block from where the last one ended. Everything a page
//! draws is kept (see [`ops`]) and written by krilla when the document ends.

pub mod drawing;
pub mod fonts;
pub mod graphics;
pub mod images;
pub mod links;
pub mod ops;
pub mod standard;
pub mod tagging;
pub mod text;

use std::collections::HashMap;
use std::fmt;

use krilla::configure::{Accessibility, Archival, ConfigurationBuilder, PdfVersion};
use krilla::metadata::{DateTime, Metadata, PageLayout};
use krilla::page::PageSettings;
use krilla::SerializeSettings;
use lopdf::{Dictionary, Object};

use fonts::LoadedFont;
use graphics::{GraphicsState, PaintBook};
use ops::Op;

/// A failure with the code the TypeScript side raises it under.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposeError {
    pub code: &'static str,
    pub message: String,
}

impl ComposeError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

/// A PDF text string: PDFDocEncoding agrees with ASCII, anything beyond it
/// is written as UTF-16BE behind its byte order mark.
fn text_string(value: &str) -> Object {
    if value.is_ascii() {
        return Object::string_literal(value);
    }
    let mut bytes = vec![0xfe, 0xff];
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, lopdf::StringFormat::Hexadecimal)
}

fn invalid_options(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_OPTIONS", message)
}

/// Distances from each edge, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    pub margins: Sides,
    /// Written as `/UserUnit` when not 1.
    pub user_unit: f32,
}

impl PageSetup {
    fn check(&self) -> Result<(), ComposeError> {
        let finite = [
            self.width,
            self.height,
            self.margins.top,
            self.margins.right,
            self.margins.bottom,
            self.margins.left,
            self.user_unit,
        ]
        .iter()
        .all(|value| value.is_finite());
        if !finite {
            return Err(invalid_options("page dimensions must be finite numbers"));
        }
        // 14 400 points is the largest page PDF 1.7 allows (Annex C).
        if !(1.0..=14_400.0).contains(&self.width) || !(1.0..=14_400.0).contains(&self.height) {
            return Err(invalid_options(format!(
                "a page of {} x {} points is outside 1 to 14400",
                self.width, self.height
            )));
        }
        if self.user_unit <= 0.0 || self.user_unit > 75_000.0 {
            return Err(invalid_options(
                "userUnit must be above 0 and at most 75000",
            ));
        }
        Ok(())
    }
}

pub struct Page {
    pub setup: PageSetup,
    pub ops: Vec<Op>,
    pub links: Vec<links::LinkRecord>,
}

impl Page {
    /// The lowest y content may reach before the bottom margin.
    pub fn max_y(&self) -> f32 {
        self.setup.height - self.setup.margins.bottom
    }
}

/// A date as the document info carries it, in UTC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instant {
    /// Milliseconds since the Unix epoch.
    pub unix_ms: f64,
}

impl Instant {
    /// `(year, month, day, hour, minute, second)`, in UTC.
    fn civil(self) -> Option<(i64, u8, u8, u8, u8, u8)> {
        if !self.unix_ms.is_finite() {
            return None;
        }
        let seconds = (self.unix_ms / 1000.0).floor() as i64;
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        // Howard Hinnant's days-to-civil.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let year = yoe + era * 400 + i64::from(month <= 2);
        Some((
            year,
            month,
            day,
            (rest / 3600) as u8,
            (rest % 3600 / 60) as u8,
            (rest % 60) as u8,
        ))
    }

    fn krilla(self) -> Option<DateTime> {
        let (year, month, day, hour, minute, second) = self.civil()?;
        let year = u16::try_from(year).ok().filter(|year| *year <= 9999)?;
        Some(
            DateTime::new(year)
                .month(month)
                .day(day)
                .hour(hour)
                .minute(minute)
                .second(second)
                .utc_offset_hour(0)
                .utc_offset_minute(0),
        )
    }

    /// `D:YYYYMMDDHHmmSSZ`, the form a PDF date string takes.
    fn pdf_string(self) -> Option<String> {
        let (year, month, day, hour, minute, second) = self.civil()?;
        (0..=9999)
            .contains(&year)
            .then(|| format!("D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z"))
    }
}

/// The document information dictionary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<Instant>,
    pub mod_date: Option<Instant>,
    /// Any other key, written as a text string.
    pub custom: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSettings {
    pub info: Info,
    pub lang: Option<String>,
    pub display_title: bool,
    /// pdfkit's spelling: `singlePage`, `oneColumn`, `twoColumnLeft`…
    pub page_layout: Option<String>,
    /// `1.4` to `2.0`.
    pub pdf_version: Option<String>,
    pub compress: bool,
    /// Whether pages stay open to [`Composer::switch_to_page`] until the end.
    pub buffer_pages: bool,
    /// Write a structure tree.
    pub tagged: bool,
    /// The standard the document is checked against as it is written.
    pub subset: Option<Subset>,
}

/// pdfkit's `subset`: PDF/A parts and levels, and PDF/UA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subset {
    PdfA1A,
    PdfA1B,
    PdfA2A,
    PdfA2B,
    PdfA2U,
    PdfA3A,
    PdfA3B,
    PdfA3U,
    PdfUa1,
}

impl Subset {
    /// pdfkit's spelling: `PDF/A-1`, `PDF/A-1a`, `PDF/A-2b`…, `PDF/UA`. A
    /// part without a level is level B, as in pdfkit.
    pub fn parse(name: &str) -> Result<Self, ComposeError> {
        Ok(match name {
            "PDF/A-1" | "PDF/A-1b" => Self::PdfA1B,
            "PDF/A-1a" => Self::PdfA1A,
            "PDF/A-2" | "PDF/A-2b" => Self::PdfA2B,
            "PDF/A-2a" => Self::PdfA2A,
            "PDF/A-2u" => Self::PdfA2U,
            "PDF/A-3" | "PDF/A-3b" => Self::PdfA3B,
            "PDF/A-3a" => Self::PdfA3A,
            "PDF/A-3u" => Self::PdfA3U,
            "PDF/UA" | "PDF/UA-1" => Self::PdfUa1,
            other => {
                return Err(invalid_options(format!(
                    "subset {other:?} is not one of PDF/A-1, 1a, 1b, 2, 2a, 2b, 2u, 3, 3a, 3b, 3u, PDF/UA"
                )))
            }
        })
    }

    fn is_archival(self) -> bool {
        self != Self::PdfUa1
    }

    /// Levels A and PDF/UA require a structure tree.
    fn needs_tags(self) -> bool {
        matches!(
            self,
            Self::PdfA1A | Self::PdfA2A | Self::PdfA3A | Self::PdfUa1
        )
    }
}

impl Default for DocumentSettings {
    fn default() -> Self {
        Self {
            info: Info::default(),
            lang: None,
            display_title: false,
            page_layout: None,
            pdf_version: None,
            compress: true,
            buffer_pages: false,
            tagged: false,
            subset: None,
        }
    }
}

/// `None` lets the standard a document is written to choose: PDF/A-1 is
/// PDF 1.4, PDF/A-2 and 3 are 1.7.
fn pdf_version(version: Option<&str>) -> Result<Option<PdfVersion>, ComposeError> {
    let Some(version) = version else {
        return Ok(None);
    };
    let version = match version {
        "1.4" => PdfVersion::Pdf14,
        "1.5" => PdfVersion::Pdf15,
        "1.6" => PdfVersion::Pdf16,
        "1.7" => PdfVersion::Pdf17,
        "2.0" => PdfVersion::Pdf20,
        other => {
            return Err(invalid_options(format!(
                "pdfVersion {other:?} is not one of 1.4, 1.5, 1.6, 1.7, 2.0"
            )))
        }
    };
    Ok(Some(version))
}

fn archival(subset: Subset) -> Option<Archival> {
    Some(match subset {
        Subset::PdfA1A => Archival::A1_A,
        Subset::PdfA1B => Archival::A1_B,
        Subset::PdfA2A => Archival::A2_A,
        Subset::PdfA2B => Archival::A2_B,
        Subset::PdfA2U => Archival::A2_U,
        Subset::PdfA3A => Archival::A3_A,
        Subset::PdfA3B => Archival::A3_B,
        Subset::PdfA3U => Archival::A3_U,
        Subset::PdfUa1 => return None,
    })
}

fn page_layout(name: &str) -> Result<PageLayout, ComposeError> {
    match name {
        "singlePage" => Ok(PageLayout::SinglePage),
        "oneColumn" => Ok(PageLayout::OneColumn),
        "twoColumnLeft" => Ok(PageLayout::TwoColumnLeft),
        "twoColumnRight" => Ok(PageLayout::TwoColumnRight),
        "twoPageLeft" => Ok(PageLayout::TwoPageLeft),
        "twoPageRight" => Ok(PageLayout::TwoPageRight),
        other => Err(invalid_options(format!(
            "pageLayout {other:?} is not one of singlePage, oneColumn, twoColumnLeft, \
             twoColumnRight, twoPageLeft, twoPageRight"
        ))),
    }
}

/// A document being written.
pub struct Composer {
    settings: DocumentSettings,
    version: Option<PdfVersion>,
    layout: Option<PageLayout>,
    pages: Vec<Page>,
    current: Option<usize>,
    /// Pages before this index are closed to `switch_to_page`.
    flushed: usize,
    pub x: f32,
    pub y: f32,
    pub(crate) fonts: Vec<LoadedFont>,
    pub(crate) font: Option<usize>,
    pub(crate) font_size: f32,
    pub(crate) state: GraphicsState,
    /// What `save()` stacked, the latest on top.
    pub(crate) saved: Vec<GraphicsState>,
    pub(crate) paints: PaintBook,
    pub(crate) destinations: Vec<links::NamedPoint>,
    pub(crate) images: Vec<images::LoadedImage>,
    pub(crate) link_count: usize,
    pub(crate) marks: Vec<tagging::Mark>,
    pub(crate) open_mark: Option<usize>,
    pub(crate) structure: Vec<tagging::Element>,
    /// What `text()` and `list()` marked, for the caller's structure tree.
    pub(crate) marked_contents: Vec<usize>,
    pub(crate) outline: Vec<links::OutlineItem>,
    /// The document's `lineGap(n)`.
    pub line_gap: f32,
    /// Word widths in thousandths of an em, by font.
    pub(crate) width_cache: HashMap<(usize, String), f32>,
    /// The wrapper a continued text hands to the next one.
    pub(crate) wrapper: Option<text::Wrapper>,
    /// Texts waiting for a page, the latest on top.
    pub(crate) suspended: Vec<text::TextRun>,
    ended: bool,
}

impl Composer {
    pub fn new(settings: DocumentSettings) -> Result<Self, ComposeError> {
        let version = pdf_version(settings.pdf_version.as_deref())?;
        let layout = settings
            .page_layout
            .as_deref()
            .map(page_layout)
            .transpose()?;
        Ok(Self {
            settings,
            version,
            layout,
            pages: Vec::new(),
            current: None,
            flushed: 0,
            x: 0.0,
            y: 0.0,
            fonts: Vec::new(),
            font: None,
            font_size: 12.0,
            state: GraphicsState::default(),
            saved: Vec::new(),
            paints: PaintBook::default(),
            destinations: Vec::new(),
            images: Vec::new(),
            link_count: 0,
            marks: Vec::new(),
            open_mark: None,
            structure: Vec::new(),
            marked_contents: Vec::new(),
            outline: Vec::new(),
            line_gap: 0.0,
            width_cache: HashMap::new(),
            wrapper: None,
            suspended: Vec::new(),
            ended: false,
        })
    }

    fn open(&self) -> Result<(), ComposeError> {
        if self.ended {
            Err(ComposeError::new(
                "DOCUMENT_ENDED",
                "the document has ended; nothing more can be added to it",
            ))
        } else {
            Ok(())
        }
    }

    pub fn add_page(&mut self, setup: PageSetup) -> Result<(), ComposeError> {
        self.open()?;
        setup.check()?;
        if !self.settings.buffer_pages {
            self.flushed = self.pages.len();
        }
        // A marked sequence ends with its page; the caller carries it on
        // with `continue_mark` on the next, as pdfkit does.
        if self.open_mark.is_some() && self.current.is_some() {
            self.end_mark()?;
        }
        self.pages.push(Page {
            setup,
            ops: Vec::new(),
            links: Vec::new(),
        });
        self.current = Some(self.pages.len() - 1);
        self.x = setup.margins.left;
        self.y = setup.margins.top;
        // A page starts untransformed and unclipped, with nothing saved;
        // colours, opacities and the stroke style carry over.
        self.state.ctm = graphics::IDENTITY;
        self.state.clips = std::sync::Arc::new(Vec::new());
        self.saved.clear();
        Ok(())
    }

    pub fn page(&self) -> Result<&Page, ComposeError> {
        self.current
            .and_then(|index| self.pages.get(index))
            .ok_or_else(|| {
                ComposeError::new("NO_PAGE", "there is no current page: call addPage() first")
            })
    }

    pub(crate) fn page_mut(&mut self) -> Result<&mut Page, ComposeError> {
        self.open()?;
        let index = self.current.ok_or_else(|| {
            ComposeError::new("NO_PAGE", "there is no current page: call addPage() first")
        })?;
        self.pages
            .get_mut(index)
            .ok_or_else(|| ComposeError::new("NO_PAGE", "the current page is gone"))
    }

    /// Change a page's margins: a footer is commonly written by dropping the
    /// bottom margin for a moment, as pdfkit lets `page.margins` be set.
    pub fn set_margins(&mut self, index: usize, margins: Sides) -> Result<(), ComposeError> {
        self.open()?;
        let finite = [margins.top, margins.right, margins.bottom, margins.left]
            .iter()
            .all(|value| value.is_finite());
        if !finite {
            return Err(invalid_options("margins must be finite numbers"));
        }
        let page = self.pages.get_mut(index).ok_or_else(|| {
            ComposeError::new("PAGE_OUT_OF_RANGE", format!("there is no page {index}"))
        })?;
        page.setup.margins = margins;
        Ok(())
    }

    /// `(start, count)`: the pages still open to [`Self::switch_to_page`].
    pub fn buffered_page_range(&self) -> (usize, usize) {
        (self.flushed, self.pages.len() - self.flushed)
    }

    pub fn switch_to_page(&mut self, index: usize) -> Result<(), ComposeError> {
        self.open()?;
        let (start, count) = self.buffered_page_range();
        if index < start || index >= start + count {
            return Err(ComposeError::new(
                "PAGE_OUT_OF_RANGE",
                format!(
                    "switchToPage({index}) is out of bounds, current buffer covers pages {start} to {}",
                    (start + count).saturating_sub(1)
                ),
            ));
        }
        self.current = Some(index);
        Ok(())
    }

    /// Close every page written so far, the current one included.
    pub fn flush_pages(&mut self) -> Result<(), ComposeError> {
        self.open()?;
        self.flushed = self.pages.len();
        self.current = None;
        Ok(())
    }

    /// Load a font and answer its handle. `standard` names the standard face
    /// these bytes stand in for, whose vertical metrics they take. The same
    /// bytes loaded twice answer the same handle, so a font is embedded once.
    pub fn load_font(
        &mut self,
        bytes: Vec<u8>,
        family: Option<&str>,
        standard: Option<&str>,
    ) -> Result<usize, ComposeError> {
        self.open()?;
        let face = match standard {
            Some(name) => Some(standard::standard(name).ok_or_else(|| {
                ComposeError::new("INVALID_FONT", format!("{name:?} is not a standard font"))
            })?),
            None => None,
        };
        let loaded = LoadedFont::load(bytes, family, face)?;
        if let Some(index) = self
            .fonts
            .iter()
            .position(|font| font.is_same(&loaded) && font.vertical == loaded.vertical)
        {
            return Ok(index);
        }
        self.fonts.push(loaded);
        Ok(self.fonts.len() - 1)
    }

    pub fn set_font(&mut self, index: usize) -> Result<(), ComposeError> {
        self.open()?;
        if index >= self.fonts.len() {
            return Err(ComposeError::new("INVALID_FONT", "no font has this handle"));
        }
        self.font = Some(index);
        Ok(())
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn set_font_size(&mut self, size: f32) -> Result<(), ComposeError> {
        if !size.is_finite() || !(0.0..=10_000.0).contains(&size) {
            return Err(invalid_options(format!(
                "a font size of {size} is outside 0 to 10000"
            )));
        }
        self.font_size = size;
        Ok(())
    }

    /// Replace the document information: it is written at the end, so it
    /// may change until then.
    pub fn set_info(&mut self, info: Info) -> Result<(), ComposeError> {
        self.open()?;
        self.settings.info = info;
        Ok(())
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn current_page_index(&self) -> Option<usize> {
        self.current
    }

    /// Write the document. It cannot be added to afterwards.
    pub fn end(&mut self) -> Result<Vec<u8>, ComposeError> {
        self.open()?;
        if self.pages.is_empty() {
            return Err(ComposeError::new(
                "NO_PAGE",
                "a document needs at least one page",
            ));
        }
        if self.open_mark.is_some() {
            self.end_mark()?;
        }
        self.check_subset()?;
        self.ended = true;

        let mut builder = ConfigurationBuilder::new();
        if let Some(version) = self.version {
            builder = builder.with_version(version);
        }
        let subset = self.settings.subset;
        if let Some(archival) = subset.and_then(archival) {
            builder = builder.with_archival_validator(archival);
        }
        if subset == Some(Subset::PdfUa1) {
            builder = builder.with_accessibility_validator(Accessibility::UA1);
        }
        let configuration = builder.finish().map_err(|error| {
            invalid_options(format!(
                "pdfVersion cannot be written as {}: {error:?}",
                self.settings
                    .subset
                    .map_or("asked", |_| "the subset requires")
            ))
        })?;
        let tagged = self.settings.tagged || subset.is_some_and(Subset::needs_tags);
        let settings = SerializeSettings {
            compress_content_streams: self.settings.compress,
            configuration,
            enable_tagging: tagged,
            ..SerializeSettings::default()
        };
        let annotations = (0..self.pages.len())
            .map(|index| self.annotations(index))
            .collect::<Result<Vec<_>, _>>()?;
        let mut identifiers = tagging::Identifiers {
            marks: vec![Vec::new(); self.marks.len()],
            links: vec![None; self.link_count],
        };
        let mut document = krilla::Document::new_with(settings);
        for (page, annotations) in self.pages.iter().zip(annotations) {
            let page_settings = PageSettings::from_wh(page.setup.width, page.setup.height)
                .ok_or_else(|| invalid_options("page size out of range"))?;
            let mut writer = document.start_page_with(page_settings);
            let mut surface = writer.surface();
            let marked = ops::replay(
                &mut surface,
                &page.ops,
                &ops::Resources {
                    fonts: &self.fonts,
                    images: &self.images,
                    paints: &self.paints,
                },
                &self.marks,
            );
            surface.finish();
            for (id, identifier) in marked {
                if let Some(sequences) = identifiers.marks.get_mut(id) {
                    sequences.push(identifier);
                }
            }
            for (id, annotation) in annotations {
                if tagged {
                    let identifier = writer.add_tagged_annotation(annotation);
                    if let Some(slot) = identifiers.links.get_mut(id) {
                        *slot = Some(identifier);
                    }
                } else {
                    writer.add_annotation(annotation);
                }
            }
            writer.finish();
        }
        if tagged {
            let tree =
                tagging::tag_tree(&self.structure, &identifiers, self.settings.lang.clone())?;
            document.set_tag_tree(tree);
        }
        for destination in self.named_destinations() {
            document
                .register_named_destination(destination)
                .ok_or_else(|| {
                    ComposeError::new("WRITE_FAILED", "a destination name is used twice")
                })?;
        }
        if let Some(outline) = self.krilla_outline()? {
            document.set_outline(outline);
        }
        document.set_metadata(self.metadata());
        let bytes = document.finish().map_err(|error| match error {
            krilla::error::KrillaError::Validation(errors) => ComposeError::new(
                "VALIDATION",
                format!(
                    "the document does not meet its subset: {}",
                    errors
                        .iter()
                        .map(|(error, _)| format!("{error:?}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            ),
            other => ComposeError::new("WRITE_FAILED", other.to_string()),
        })?;
        self.finishing_pass(bytes)
    }

    /// What PDF/A forbids that krilla does not check: the information this
    /// pass would add to the Info dictionary has no XMP counterpart, and the
    /// two must agree.
    fn check_subset(&self) -> Result<(), ComposeError> {
        let Some(subset) = self.settings.subset else {
            return Ok(());
        };
        if !subset.is_archival() {
            return Ok(());
        }
        let info = &self.settings.info;
        if !info.custom.is_empty() {
            return Err(ComposeError::new(
                "VALIDATION",
                "PDF/A keeps the information dictionary and the XMP metadata in step; information keys of your own cannot be written",
            ));
        }
        if info.mod_date.is_some() {
            return Err(ComposeError::new(
                "VALIDATION",
                "PDF/A keeps the information dictionary and the XMP metadata in step; a modification date of its own cannot be written",
            ));
        }
        Ok(())
    }

    /// Whether images may be smoothed when scaled: PDF/A forbids it.
    pub(crate) fn interpolate_images(&self) -> bool {
        self.settings
            .subset
            .is_none_or(|subset| !subset.is_archival())
    }

    fn metadata(&self) -> Metadata {
        let info = &self.settings.info;
        let mut metadata = Metadata::new()
            .producer(info.producer.clone().unwrap_or_else(|| "Vellum".into()))
            .creator(info.creator.clone().unwrap_or_else(|| "Vellum".into()));
        if let Some(title) = &info.title {
            metadata = metadata.title(title.clone());
        }
        if let Some(author) = &info.author {
            metadata = metadata.authors(vec![author.clone()]);
        }
        if let Some(subject) = &info.subject {
            metadata = metadata.description(subject.clone());
        }
        if let Some(keywords) = &info.keywords {
            metadata = metadata.keywords(vec![keywords.clone()]);
        }
        if let Some(date) = info.creation_date.and_then(Instant::krilla) {
            metadata = metadata.creation_date(date);
        }
        if let Some(lang) = &self.settings.lang {
            metadata = metadata.language(lang.clone());
        }
        if let Some(layout) = self.layout {
            metadata = metadata.page_layout(layout);
        }
        metadata
    }

    /// What krilla does not write: `/UserUnit`, `DisplayDocTitle`, a
    /// modification date of its own, and information keys of the caller's.
    /// Skipped, and the bytes left as krilla wrote them, when none is asked.
    fn finishing_pass(&self, bytes: Vec<u8>) -> Result<Vec<u8>, ComposeError> {
        let info = &self.settings.info;
        let units: Vec<(usize, f32)> = self
            .pages
            .iter()
            .enumerate()
            .filter(|(_, page)| page.setup.user_unit != 1.0)
            .map(|(index, page)| (index, page.setup.user_unit))
            .collect();
        let mod_date = info.mod_date.and_then(Instant::pdf_string);
        let outlined = !self.outline.is_empty();
        if units.is_empty()
            && !self.settings.display_title
            && !outlined
            && mod_date.is_none()
            && info.custom.is_empty()
        {
            return Ok(bytes);
        }

        let failed = |error: lopdf::Error| ComposeError::new("WRITE_FAILED", error.to_string());
        let mut document = lopdf::Document::load_mem(&bytes).map_err(failed)?;
        let pages = document.get_pages();
        for (index, unit) in units {
            let Some(&page_id) = pages.get(&(index as u32 + 1)) else {
                continue;
            };
            if let Ok(page) = document
                .get_object_mut(page_id)
                .and_then(|object| object.as_dict_mut())
            {
                page.set("UserUnit", Object::Real(unit));
            }
        }
        if self.settings.display_title {
            let catalog = document.catalog_mut().map_err(failed)?;
            let mut preferences = Dictionary::new();
            preferences.set("DisplayDocTitle", Object::Boolean(true));
            catalog.set("ViewerPreferences", Object::Dictionary(preferences));
        }
        // As pdfkit does: a document with bookmarks opens showing them.
        if outlined {
            let catalog = document.catalog_mut().map_err(failed)?;
            if catalog.get(b"PageMode").is_err() {
                catalog.set("PageMode", Object::Name(b"UseOutlines".to_vec()));
            }
        }
        if mod_date.is_some() || !info.custom.is_empty() {
            let info_id = match document.trailer.get(b"Info") {
                Ok(Object::Reference(id)) => *id,
                _ => {
                    let id = document.add_object(Dictionary::new());
                    document.trailer.set("Info", Object::Reference(id));
                    id
                }
            };
            let dictionary = document
                .get_object_mut(info_id)
                .and_then(|object| object.as_dict_mut())
                .map_err(failed)?;
            if let Some(date) = mod_date {
                dictionary.set("ModDate", Object::string_literal(date));
            }
            for (key, value) in &info.custom {
                dictionary.set(key.as_str(), text_string(value));
            }
        }
        let mut out = Vec::new();
        document
            .save_to(&mut out)
            .map_err(|error| ComposeError::new("WRITE_FAILED", error.to_string()))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn composer(settings: DocumentSettings) -> Composer {
        Composer::new(settings).expect("settings are valid")
    }

    #[test]
    fn writes_the_pages_it_was_given() {
        let mut document = composer(DocumentSettings::default());
        document.add_page(A4).unwrap();
        document
            .add_page(PageSetup {
                width: 612.0,
                height: 792.0,
                ..A4
            })
            .unwrap();
        let bytes = document.end().unwrap();
        let info = crate::inspect(&bytes).unwrap();
        assert_eq!(info.page_count, 2);
        let sizes = crate::page_dimensions(&bytes).unwrap();
        assert!((sizes[0].width - 595.28).abs() < 0.01);
        assert!((sizes[0].height - 841.89).abs() < 0.01);
        assert!((sizes[1].width - 612.0).abs() < 0.01);
    }

    #[test]
    fn the_cursor_starts_at_the_margins_of_each_page() {
        let mut document = composer(DocumentSettings::default());
        document
            .add_page(PageSetup {
                margins: Sides {
                    top: 10.0,
                    right: 20.0,
                    bottom: 30.0,
                    left: 40.0,
                },
                ..A4
            })
            .unwrap();
        assert_eq!((document.x, document.y), (40.0, 10.0));
        document.x = 300.0;
        document.add_page(A4).unwrap();
        assert_eq!((document.x, document.y), (72.0, 72.0));
        assert!((document.page().unwrap().max_y() - (841.89 - 72.0)).abs() < 1e-3);
    }

    #[test]
    fn only_buffered_pages_can_be_revisited() {
        let mut unbuffered = composer(DocumentSettings::default());
        unbuffered.add_page(A4).unwrap();
        unbuffered.add_page(A4).unwrap();
        assert_eq!(unbuffered.buffered_page_range(), (1, 1));
        assert_eq!(
            unbuffered.switch_to_page(0).unwrap_err().code,
            "PAGE_OUT_OF_RANGE"
        );

        let mut buffered = composer(DocumentSettings {
            buffer_pages: true,
            ..DocumentSettings::default()
        });
        for _ in 0..3 {
            buffered.add_page(A4).unwrap();
        }
        assert_eq!(buffered.buffered_page_range(), (0, 3));
        buffered.switch_to_page(0).unwrap();
        assert_eq!(buffered.current_page_index(), Some(0));
        assert_eq!(
            buffered.switch_to_page(3).unwrap_err().code,
            "PAGE_OUT_OF_RANGE"
        );

        buffered.flush_pages().unwrap();
        assert_eq!(buffered.buffered_page_range(), (3, 0));
        assert_eq!(
            buffered.page().err().map(|error| error.code),
            Some("NO_PAGE")
        );
    }

    #[test]
    fn nothing_is_added_after_the_end() {
        let mut document = composer(DocumentSettings::default());
        document.add_page(A4).unwrap();
        document.end().unwrap();
        assert_eq!(document.add_page(A4).unwrap_err().code, "DOCUMENT_ENDED");
        assert_eq!(document.end().unwrap_err().code, "DOCUMENT_ENDED");
    }

    #[test]
    fn a_document_without_pages_is_refused() {
        let mut document = composer(DocumentSettings::default());
        assert_eq!(document.end().unwrap_err().code, "NO_PAGE");
    }

    #[test]
    fn refuses_impossible_pages() {
        let mut document = composer(DocumentSettings::default());
        for setup in [
            PageSetup { width: 0.0, ..A4 },
            PageSetup {
                height: f32::NAN,
                ..A4
            },
            PageSetup {
                width: 20_000.0,
                ..A4
            },
            PageSetup {
                user_unit: 0.0,
                ..A4
            },
        ] {
            assert_eq!(
                document.add_page(setup).unwrap_err().code,
                "INVALID_OPTIONS"
            );
        }
    }

    #[test]
    fn refuses_unknown_versions_and_layouts() {
        let version = Composer::new(DocumentSettings {
            pdf_version: Some("1.3".into()),
            ..DocumentSettings::default()
        });
        assert_eq!(
            version.err().map(|error| error.code),
            Some("INVALID_OPTIONS")
        );
        let layout = Composer::new(DocumentSettings {
            page_layout: Some("sideways".into()),
            ..DocumentSettings::default()
        });
        assert_eq!(
            layout.err().map(|error| error.code),
            Some("INVALID_OPTIONS")
        );
    }

    #[test]
    fn writes_the_document_information() {
        let mut document = composer(DocumentSettings {
            info: Info {
                title: Some("Facture 2026-041".into()),
                author: Some("Atelier Dubois".into()),
                subject: Some("Rénovation".into()),
                keywords: Some("facture, rénovation".into()),
                creation_date: Some(Instant {
                    unix_ms: 1_791_980_850_000.0,
                }),
                mod_date: Some(Instant {
                    unix_ms: 1_791_990_000_000.0,
                }),
                custom: vec![("Dossier".into(), "Ç-42".into())],
                ..Info::default()
            },
            lang: Some("fr-CH".into()),
            display_title: true,
            ..DocumentSettings::default()
        });
        document
            .add_page(PageSetup {
                user_unit: 2.0,
                ..A4
            })
            .unwrap();
        let bytes = document.end().unwrap();

        let metadata = crate::metadata(&bytes).unwrap();
        assert_eq!(metadata.title.as_deref(), Some("Facture 2026-041"));
        assert_eq!(metadata.author.as_deref(), Some("Atelier Dubois"));
        assert_eq!(metadata.subject.as_deref(), Some("Rénovation"));
        assert_eq!(metadata.producer.as_deref(), Some("Vellum"));

        let parsed = lopdf::Document::load_mem(&bytes).unwrap();
        let info_id = parsed.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = parsed.get_dictionary(info_id).unwrap();
        assert_eq!(
            info.get(b"ModDate").unwrap().as_str().unwrap(),
            b"D:20261014150000Z"
        );
        assert!(info.get(b"Dossier").is_ok());
        let catalog = parsed.catalog().unwrap();
        assert_eq!(catalog.get(b"Lang").unwrap().as_str().unwrap(), b"fr-CH");
        let preferences = catalog
            .get(b"ViewerPreferences")
            .unwrap()
            .as_dict()
            .unwrap();
        assert!(preferences
            .get(b"DisplayDocTitle")
            .unwrap()
            .as_bool()
            .unwrap());
        let page_id = *parsed.get_pages().get(&1).unwrap();
        let page = parsed.get_dictionary(page_id).unwrap();
        assert_eq!(page.get(b"UserUnit").unwrap().as_float().unwrap(), 2.0);
    }

    #[test]
    fn converts_dates_to_utc_civil_time() {
        let instant = Instant { unix_ms: 0.0 };
        assert_eq!(instant.pdf_string().as_deref(), Some("D:19700101000000Z"));
        let leap = Instant {
            unix_ms: 951_782_400_000.0,
        };
        assert_eq!(leap.pdf_string().as_deref(), Some("D:20000229000000Z"));
        assert_eq!(Instant { unix_ms: f64::NAN }.pdf_string(), None);
    }
}
