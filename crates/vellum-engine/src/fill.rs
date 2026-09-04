//! Filling a document's interactive form.
//!
//! Writing `/V` is the easy half and the useless one on its own: most readers
//! paint a field from its APPEARANCE STREAM (`/AP`), not from its value, so a
//! document filled without regenerating them opens looking empty while holding
//! every answer. That is the trap this module exists to close.
//!
//! What each kind needs:
//!
//! - text and choice fields need an appearance stream built from scratch;
//! - a checkbox or radio already ships one appearance per state, so only the
//!   widget's `/AS` has to point at the right one.

use std::collections::BTreeMap;

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

use crate::form::{fields_of, FieldKind};
use crate::metrics::width_of;
use crate::stamp_text::{escape_pdf_literal, to_win_ansi, StandardFont};

/// What a caller asks to be written into one field.
pub struct FieldValue {
    pub name: String,
    pub value: String,
}

/// The appearance of a filled field, as `/DA` declares it.
#[derive(Debug, Clone, Copy)]
struct Appearance {
    size: f32,
    color: [f32; 3],
    /// `/DA` asked for size 0, meaning "whatever fits the box".
    auto_size: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            size: 10.0,
            color: [0.0, 0.0, 0.0],
            auto_size: false,
        }
    }
}

/// Read the type size and colour out of a `/DA` string.
///
/// `/DA` is a fragment of content stream, conventionally something like
/// `/Helv 9 Tf 0 g`. Only the size and the colour are taken from it: the font
/// it names lives in the form's `/DR`, which a document may or may not have
/// filled in, so the appearance stream below carries a standard font of its
/// own instead of trusting it.
fn parse_default_appearance(da: &str) -> Appearance {
    let mut appearance = Appearance::default();
    let tokens: Vec<&str> = da.split_whitespace().collect();

    for (index, token) in tokens.iter().enumerate() {
        match *token {
            "Tf" => {
                if let Some(size) = tokens
                    .get(index.wrapping_sub(1))
                    .and_then(|s| s.parse::<f32>().ok())
                {
                    // Size 0 means "fit the box", which is resolved once the
                    // widget's rectangle and the text are both known.
                    if size > 0.0 {
                        appearance.size = size;
                    } else if size == 0.0 {
                        appearance.auto_size = true;
                    }
                }
            }
            "g" => {
                if let Some(grey) = tokens
                    .get(index.wrapping_sub(1))
                    .and_then(|s| s.parse::<f32>().ok())
                {
                    appearance.color = [grey, grey, grey];
                }
            }
            "rg" => {
                let channel = |offset: usize| {
                    tokens
                        .get(index.wrapping_sub(offset))
                        .and_then(|s| s.parse::<f32>().ok())
                };
                if let (Some(red), Some(green), Some(blue)) = (channel(3), channel(2), channel(1)) {
                    appearance.color = [red, green, blue];
                }
            }
            _ => {}
        }
    }

    appearance
}

/// The `/DA` in force for a field: its own, else the form's.
fn default_appearance(document: &Document, field_id: ObjectId) -> Appearance {
    let own = document
        .get_dictionary(field_id)
        .ok()
        .and_then(|field| field.get(b"DA").ok())
        .and_then(|da| da.as_str().ok())
        .map(|bytes| String::from_utf8_lossy(bytes).to_string());

    let form = document
        .catalog()
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok())
        .and_then(|form| match form {
            Object::Reference(id) => document.get_dictionary(*id).ok(),
            Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        })
        .and_then(|form| form.get(b"DA").ok())
        .and_then(|da| da.as_str().ok())
        .map(|bytes| String::from_utf8_lossy(bytes).to_string());

    own.or(form)
        .map(|da| parse_default_appearance(&da))
        .unwrap_or_default()
}

/// The quadding in force for a field: its own, else the form's.
///
/// 0 is left, 1 centred, 2 right (§12.7.4.3). Like the type and the flags, it
/// is inherited down `/Parent`, so a field commonly declares none itself.
fn quadding_of(document: &Document, field_id: ObjectId) -> i64 {
    let own = document
        .get_dictionary(field_id)
        .ok()
        .and_then(|field| crate::form::inherited(document, field, b"Q"))
        .and_then(|quadding| quadding.as_i64().ok());

    let form = document
        .catalog()
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok())
        .and_then(|form| match form {
            Object::Reference(id) => document.get_dictionary(*id).ok(),
            Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        })
        .and_then(|form| form.get(b"Q").ok())
        .and_then(|quadding| quadding.as_i64().ok());

    own.or(form).unwrap_or(0)
}

/// The widgets that draw a field: the field itself when they are merged, or
/// its kids when they are not.
pub(crate) fn widgets_of(document: &Document, field_id: ObjectId) -> Vec<ObjectId> {
    let Ok(field) = document.get_dictionary(field_id) else {
        return Vec::new();
    };

    match field.get(b"Kids") {
        Ok(Object::Array(kids)) => {
            let widgets: Vec<ObjectId> = kids
                .iter()
                .filter_map(|kid| {
                    let id = kid.as_reference().ok()?;
                    // A kid with /T is another field, not a widget.
                    let kid_dictionary = document.get_dictionary(id).ok()?;
                    kid_dictionary.get(b"T").err().map(|_| id)
                })
                .collect();
            if widgets.is_empty() {
                vec![field_id]
            } else {
                widgets
            }
        }
        _ => vec![field_id],
    }
}

/// The size of a widget's rectangle.
fn widget_size(document: &Document, widget_id: ObjectId) -> Option<(f32, f32)> {
    let rect = document
        .get_dictionary(widget_id)
        .ok()?
        .get(b"Rect")
        .ok()?
        .as_array()
        .ok()?;
    let value =
        |index: usize| -> Option<f32> { rect.get(index).and_then(|number| number.as_float().ok()) };
    let (x0, y0, x1, y1) = (value(0)?, value(1)?, value(2)?, value(3)?);
    Some(((x1 - x0).abs(), (y1 - y0).abs()))
}

/// The font every appearance stream we build carries. Measuring with anything
/// else would be guessing: the reader lays the text out with the font the
/// stream names, and this is the one it names.
const FONT: StandardFont = StandardFont::Helvetica;

const PADDING: f32 = 2.0;
/// Baseline-to-baseline, as a multiple of the type size.
const LEADING: f32 = 1.2;

/// Break a paragraph into lines that fit `available` points.
///
/// Words are kept whole where they can be. A word too long for the line on its
/// own is broken across lines rather than allowed to run past the edge —
/// truncating it would silently lose part of the answer.
fn wrap(paragraph: &[u8], size: f32, available: f32) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();

    for word in paragraph.split(|byte| *byte == b' ') {
        let mut word = word;
        loop {
            let separator: &[u8] = if current.is_empty() { b"" } else { b" " };
            let candidate = width_of(FONT, &current, size)
                + width_of(FONT, separator, size)
                + width_of(FONT, word, size);

            if candidate <= available {
                current.extend_from_slice(separator);
                current.extend_from_slice(word);
                break;
            }
            if !current.is_empty() {
                // Try the word again, alone on the next line.
                lines.push(std::mem::take(&mut current));
                continue;
            }

            // The word does not fit even on a line of its own, so it is cut at
            // the last character that does — at least one, or this would spin.
            let mut taken = 1;
            while taken < word.len() && width_of(FONT, &word[..taken + 1], size) <= available {
                taken += 1;
            }
            lines.push(word[..taken].to_vec());
            word = &word[taken..];
            if word.is_empty() {
                break;
            }
        }
    }

    lines.push(current);
    lines
}

/// The lines to draw, honouring the caller's own breaks and, for a multiline
/// field, the width of the box.
fn lay_out(text: &[u8], size: f32, available: f32, multiline: bool) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    for paragraph in text.split(|byte| *byte == b'\n') {
        if multiline {
            lines.extend(wrap(paragraph, size, available));
        } else {
            lines.push(paragraph.to_vec());
        }
    }
    lines
}

/// Choose a type size for a field whose `/DA` asks for one.
///
/// The specification does not say what "auto" means, so this is a heuristic:
/// start where a single line would fill the box — or at 12pt for a multiline
/// field, which is where Acrobat starts — and step down until the text fits.
fn fit_size(text: &[u8], width: f32, height: f32, multiline: bool) -> f32 {
    const STEP: f32 = 0.25;
    const FLOOR: f32 = 4.0;

    let available = (width - 2.0 * PADDING).max(1.0);
    let box_height = (height - 2.0 * PADDING).max(1.0);
    let mut size = if multiline {
        12.0
    } else {
        (box_height / LEADING).clamp(FLOOR, 72.0)
    };

    while size > FLOOR {
        let fits = if multiline {
            let lines = lay_out(text, size, available, true).len() as f32;
            lines * size * LEADING <= box_height
        } else {
            width_of(FONT, text, size) <= available
        };
        if fits {
            return size;
        }
        size -= STEP;
    }
    FLOOR
}

/// Where a line starts, for the field's quadding.
fn line_start(quadding: i64, width: f32, line: &[u8], size: f32) -> f32 {
    let drawn = width_of(FONT, line, size);
    let start = match quadding {
        1 => (width - drawn) / 2.0,
        2 => width - PADDING - drawn,
        // 0, and anything the document invents, is left-aligned.
        _ => PADDING,
    };
    // A line wider than its box starts at the edge rather than off it.
    start.max(PADDING)
}

/// Build the appearance stream that paints `text` inside a widget.
fn text_appearance(
    width: f32,
    height: f32,
    text: &[u8],
    appearance: Appearance,
    multiline: bool,
    quadding: i64,
) -> Stream {
    let [red, green, blue] = appearance.color;
    let size = if appearance.auto_size {
        fit_size(text, width, height, multiline)
    } else {
        appearance.size
    };
    let available = (width - 2.0 * PADDING).max(1.0);
    let lines = lay_out(text, size, available, multiline);

    let mut content: Vec<u8> = Vec::new();
    // /Tx BMC ... EMC marks this as a form field's appearance, which is what
    // tells a reader it may replace it when the value changes.
    content.extend_from_slice(b"/Tx BMC\nq\nBT\n");
    content
        .extend_from_slice(format!("/{FONT_KEY} {size} Tf\n{red} {green} {blue} rg\n").as_bytes());

    // A single-line field centres its text vertically; a multiline one starts
    // at the top.
    let first_baseline = if multiline {
        height - PADDING - size
    } else {
        (height - size) / 2.0 + size * 0.2
    };

    for (index, line) in lines.iter().enumerate() {
        let baseline = first_baseline - index as f32 * size * LEADING;
        let start = line_start(quadding, width, line, size);
        // Each line is positioned outright rather than stepped down with T*,
        // because its own quadding decides where it begins.
        content.extend_from_slice(format!("1 0 0 1 {start} {baseline} Tm\n").as_bytes());
        content.push(b'(');
        content.extend_from_slice(&escape_pdf_literal(line));
        content.extend_from_slice(b") Tj\n");
    }

    content.extend_from_slice(b"ET\nQ\nEMC\n");

    let font = dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    };

    Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), Object::Real(width), Object::Real(height)],
            // The stream carries its own font rather than referencing the
            // form's /DR, which a document is free to leave incomplete.
            "Resources" => dictionary! {
                "Font" => dictionary! { FONT_KEY => font },
            },
        },
        content,
    )
}

const FONT_KEY: &str = "VellumFormFont";

/// Point a checkbox or radio widget at one of the appearances it ships.
fn set_widget_state(document: &mut Document, widget_id: ObjectId, state: &str) {
    // A widget only shows a state it actually has an appearance for; anything
    // else is turned off rather than left in an undefined state.
    let has_state = document
        .get_dictionary(widget_id)
        .ok()
        .and_then(|widget| widget.get(b"AP").ok().cloned())
        .and_then(|ap| match ap {
            Object::Reference(id) => document.get_dictionary(id).ok().cloned(),
            Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        })
        .and_then(|ap| ap.get(b"N").ok().cloned())
        .and_then(|normal| match normal {
            Object::Reference(id) => document.get_dictionary(id).ok().cloned(),
            Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        })
        .is_some_and(|normal| normal.get(state.as_bytes()).is_ok());

    let chosen = if has_state { state } else { "Off" };
    if let Ok(widget) = document
        .get_object_mut(widget_id)
        .and_then(|object| object.as_dict_mut())
    {
        widget.set("AS", Object::Name(chosen.as_bytes().to_vec()));
    }
}

/// Fill the named fields with the given values.
///
/// Names are the fully qualified ones `form_fields` reports. A name the form
/// does not have is an error rather than a silent no-op: a filled document
/// missing an answer nobody noticed is worse than a failure.
pub fn fill_form(bytes: &[u8], values: &[FieldValue]) -> Result<Vec<u8>, String> {
    let mut document =
        Document::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))?;

    let known: BTreeMap<String, (ObjectId, crate::form::FormField)> = fields_of(&document)
        .into_iter()
        .map(|(id, field)| (field.name.clone(), (id, field)))
        .collect();

    for FieldValue { name, value } in values {
        let Some((field_id, field)) = known.get(name) else {
            return Err(format!(
                "the form has no field named {name:?} — it has {}",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.keys().cloned().collect::<Vec<_>>().join(", ")
                }
            ));
        };

        if field.read_only {
            return Err(format!("the field {name:?} is read-only"));
        }
        if let Some(max) = field.max_length {
            if value.chars().count() > max as usize {
                return Err(format!(
                    "{name:?} accepts at most {max} characters, got {}",
                    value.chars().count()
                ));
            }
        }

        match field.kind {
            FieldKind::PushButton => {
                return Err(format!("{name:?} is a push button and holds no value"));
            }
            FieldKind::Signature => {
                return Err(format!(
                    "{name:?} is a signature field, which this cannot fill"
                ));
            }
            FieldKind::Checkbox | FieldKind::Radio => {
                // The accepted states are the document's own; writing anything
                // else would leave the control blank without complaining.
                if value != "Off" && !field.options.contains(value) {
                    return Err(format!(
                        "{name:?} accepts {:?} or \"Off\", got {value:?}",
                        field.options
                    ));
                }
                if let Ok(field_dictionary) = document
                    .get_object_mut(*field_id)
                    .and_then(|object| object.as_dict_mut())
                {
                    field_dictionary.set("V", Object::Name(value.as_bytes().to_vec()));
                }
                for widget_id in widgets_of(&document, *field_id) {
                    set_widget_state(&mut document, widget_id, value);
                }
            }
            FieldKind::Text | FieldKind::Dropdown | FieldKind::ListBox => {
                if matches!(field.kind, FieldKind::Dropdown | FieldKind::ListBox)
                    && !field.options.is_empty()
                    && !field.options.contains(value)
                {
                    return Err(format!(
                        "{name:?} accepts {:?}, got {value:?}",
                        field.options
                    ));
                }

                let encoded = to_win_ansi(value)
                    .map_err(|error| format!("cannot write into {name:?}: {error}"))?;
                let appearance = default_appearance(&document, *field_id);
                let quadding = quadding_of(&document, *field_id);

                if let Ok(field_dictionary) = document
                    .get_object_mut(*field_id)
                    .and_then(|object| object.as_dict_mut())
                {
                    field_dictionary.set("V", lopdf::Object::string_literal(encoded.clone()));
                }

                for widget_id in widgets_of(&document, *field_id) {
                    let Some((width, height)) = widget_size(&document, widget_id) else {
                        // No /Rect means nothing is drawn for this widget;
                        // the value is still written.
                        continue;
                    };
                    let stream = text_appearance(
                        width,
                        height,
                        &encoded,
                        appearance,
                        field.multiline,
                        quadding,
                    );
                    let stream_id = document.add_object(Object::Stream(stream));
                    if let Ok(widget) = document
                        .get_object_mut(widget_id)
                        .and_then(|object| object.as_dict_mut())
                    {
                        widget.set(
                            "AP",
                            Object::Dictionary(dictionary! {
                                "N" => Object::Reference(stream_id),
                            }),
                        );
                    }
                }
            }
        }
    }

    // A belt to the braces above: readers that regenerate appearances
    // themselves are told the ones on file may be stale. It is not relied on —
    // support is uneven — but it costs nothing.
    set_need_appearances(&mut document);

    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out)
}

fn set_need_appearances(document: &mut Document) {
    let form = document
        .catalog()
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok().cloned());

    match form {
        Some(Object::Reference(id)) => {
            if let Ok(dictionary) = document
                .get_object_mut(id)
                .and_then(|object| object.as_dict_mut())
            {
                dictionary.set("NeedAppearances", true);
            }
        }
        Some(Object::Dictionary(mut dictionary)) => {
            dictionary.set("NeedAppearances", true);
            let catalog_id = document
                .trailer
                .get(b"Root")
                .and_then(Object::as_reference)
                .ok();
            if let Some(catalog_id) = catalog_id {
                if let Ok(catalog) = document
                    .get_object_mut(catalog_id)
                    .and_then(|object| object.as_dict_mut())
                {
                    catalog.set("AcroForm", Object::Dictionary(dictionary));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;

    use hayro::vello_cpu::Pixmap;
    use lopdf::dictionary;

    use super::*;
    use crate::{form_fields, render_page, ImageFormat, RenderOptions};

    fn value(name: &str, value: &str) -> FieldValue {
        FieldValue {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    /// A form whose widgets carry a `/Rect` and hang off the page's `/Annots`,
    /// so the result can actually be RENDERED — which is the only way to prove
    /// an appearance stream was regenerated.
    pub(crate) fn form_document() -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();

        let full_name = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Tx",
            "T" => lopdf::text_string("fullName"),
            "DA" => lopdf::Object::string_literal("/Helv 12 Tf 0 g"),
            "Rect" => vec![50.into(), 700.into(), 350.into(), 730.into()],
            "P" => page_id,
        });

        let notes = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Tx",
            "T" => lopdf::text_string("notes"),
            "Ff" => 1 << 12,
            "MaxLen" => 20,
            "Rect" => vec![50.into(), 600.into(), 350.into(), 680.into()],
            "P" => page_id,
        });

        let reference = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Tx",
            "T" => lopdf::text_string("reference"),
            "Ff" => 1,
            "Rect" => vec![50.into(), 560.into(), 350.into(), 590.into()],
            "P" => page_id,
        });

        let accepted = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Btn",
            "T" => lopdf::text_string("accepted"),
            "V" => Object::Name(b"Off".to_vec()),
            "Rect" => vec![50.into(), 520.into(), 70.into(), 540.into()],
            "P" => page_id,
            "AP" => dictionary! {
                "N" => dictionary! { "Yes" => Object::Null, "Off" => Object::Null },
            },
        });

        let country = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Ch",
            "T" => lopdf::text_string("country"),
            "Ff" => 1 << 17,
            "Opt" => vec![lopdf::text_string("CH"), lopdf::text_string("FR")],
            "Rect" => vec![50.into(), 470.into(), 250.into(), 500.into()],
            "P" => page_id,
        });

        let signature = document.add_object(dictionary! {
            "FT" => "Sig",
            "T" => lopdf::text_string("signature"),
        });

        let fields = vec![
            Object::Reference(full_name),
            Object::Reference(notes),
            Object::Reference(reference),
            Object::Reference(accepted),
            Object::Reference(country),
            Object::Reference(signature),
        ];
        let annots = vec![
            Object::Reference(full_name),
            Object::Reference(notes),
            Object::Reference(reference),
            Object::Reference(accepted),
            Object::Reference(country),
        ];

        document.objects.insert(
            page_id,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Annots" => annots,
            }),
        );
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => 1,
                "Kids" => vec![Object::Reference(page_id)],
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => dictionary! {
                "Fields" => fields,
                "DA" => lopdf::Object::string_literal("/Helv 0 Tf 0 g"),
            },
        });
        document.trailer.set("Root", catalog_id);

        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }

    /// Count the dark pixels inside a rectangle of the rendered page, in
    /// top-left pixel coordinates at 72 DPI.
    pub(crate) fn ink_in(pdf: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
        let png = render_page(
            pdf,
            0,
            &RenderOptions {
                format: ImageFormat::Png,
                ..Default::default()
            },
        )
        .expect("rendering should succeed");
        let pixmap = Pixmap::from_png(Cursor::new(png)).expect("the render decodes");
        let width = u32::from(pixmap.width());
        let data = pixmap.data();

        let mut dark = 0;
        for y in y0..y1.min(u32::from(pixmap.height())) {
            for x in x0..x1.min(width) {
                if data[(y * width + x) as usize].r < 128 {
                    dark += 1;
                }
            }
        }
        dark
    }

    /// A form with a single text field, so a test can say exactly what its
    /// rectangle, flags, `/DA` and quadding are.
    fn one_field(rect: [i32; 4], flags: i64, da: &str, quadding: Option<i64>) -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();

        let mut field = dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Tx",
            "T" => lopdf::text_string("answer"),
            "Ff" => flags,
            "DA" => lopdf::Object::string_literal(da),
            "Rect" => rect.iter().map(|value| (*value).into()).collect::<Vec<Object>>(),
            "P" => page_id,
        };
        if let Some(quadding) = quadding {
            field.set("Q", quadding);
        }
        let field_id = document.add_object(field);

        document.objects.insert(
            page_id,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Annots" => vec![Object::Reference(field_id)],
            }),
        );
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => 1,
                "Kids" => vec![Object::Reference(page_id)],
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(field_id)] },
        });
        document.trailer.set("Root", catalog_id);

        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }

    const MULTILINE: i64 = 1 << 12;
    /// A field at x 50..350, y 700..730 — so y 112..142 down from the top of
    /// an 842pt page.
    const LINE_RECT: [i32; 4] = [50, 700, 350, 730];

    #[test]
    fn a_right_aligned_field_puts_its_text_at_the_right_edge() {
        let filled = fill_form(
            &one_field(LINE_RECT, 0, "/Helv 12 Tf 0 g", Some(2)),
            &[value("answer", "AB")],
        )
        .expect("filling should succeed");

        assert!(
            ink_in(&filled, 300, 110, 350, 145) > 10,
            "the text belongs against the right edge"
        );
        assert_eq!(
            ink_in(&filled, 50, 110, 250, 145),
            0,
            "and nothing belongs on the left"
        );
    }

    #[test]
    fn a_centred_field_puts_its_text_in_the_middle() {
        let filled = fill_form(
            &one_field(LINE_RECT, 0, "/Helv 12 Tf 0 g", Some(1)),
            &[value("answer", "AB")],
        )
        .expect("filling should succeed");

        assert!(ink_in(&filled, 180, 110, 220, 145) > 10, "centred");
        assert_eq!(ink_in(&filled, 50, 110, 150, 145), 0, "not on the left");
        assert_eq!(ink_in(&filled, 300, 110, 350, 145), 0, "not on the right");
    }

    /// Left is the default, and it is also what a document asking for
    /// something the specification does not define should get.
    #[test]
    fn a_field_with_no_quadding_stays_on_the_left() {
        let filled = fill_form(
            &one_field(LINE_RECT, 0, "/Helv 12 Tf 0 g", None),
            &[value("answer", "AB")],
        )
        .expect("filling should succeed");

        assert!(ink_in(&filled, 50, 110, 100, 145) > 10, "against the left");
        assert_eq!(ink_in(&filled, 200, 110, 350, 145), 0, "nowhere else");
    }

    /// A multiline field now breaks a long answer across lines by itself. The
    /// value has no newline in it, so any second line is one we chose.
    #[test]
    fn a_multiline_field_wraps_a_long_answer() {
        // x 50..350, y 600..680 — so y 162..242 from the top.
        let filled = fill_form(
            &one_field([50, 600, 350, 680], MULTILINE, "/Helv 12 Tf 0 g", None),
            &[value(
                "answer",
                "Le present mandat couvre la prevoyance professionnelle du titulaire",
            )],
        )
        .expect("filling should succeed");

        assert!(ink_in(&filled, 50, 162, 350, 180) > 20, "a first line");
        assert!(ink_in(&filled, 50, 180, 350, 200) > 20, "and a second one");
    }

    /// `/DA` with a size of 0 asks for whatever fits the box. Before the
    /// metrics there was nothing to fit it with, so the default was kept and
    /// the answer overflowed.
    #[test]
    fn an_automatic_size_shrinks_until_the_answer_fits() {
        let long = b"Amelie Durand-Chevalier de la Tour du Pin";
        let size = fit_size(long, 150.0, 30.0, false);

        assert!(
            size < 10.0,
            "it had to shrink below the default, got {size}"
        );
        assert!(
            width_of(FONT, long, size) <= 150.0 - 2.0 * PADDING,
            "and the whole answer has to fit the box"
        );
    }

    /// It grows as well as shrinks: a short answer in a tall box should not be
    /// left at the size a missing `/DA` would have given it.
    #[test]
    fn an_automatic_size_fills_a_roomy_box() {
        assert!(fit_size(b"OK", 300.0, 40.0, false) > 10.0);
    }

    /// A multiline field fits by lines rather than by width, since it wraps.
    #[test]
    fn an_automatic_size_counts_the_lines_it_will_wrap_to() {
        let text = b"Le present mandat couvre la prevoyance professionnelle du titulaire";
        let roomy = fit_size(text, 300.0, 120.0, true);
        let cramped = fit_size(text, 300.0, 30.0, true);
        assert!(
            cramped < roomy,
            "less room means smaller type, got {cramped} against {roomy}"
        );
        let lines = lay_out(text, cramped, 300.0 - 2.0 * PADDING, true).len() as f32;
        assert!(
            lines * cramped * LEADING <= 30.0 - 2.0 * PADDING,
            "and what it picks has to fit"
        );
    }

    /// End to end: the automatic size still paints something on the page.
    #[test]
    fn a_field_asking_for_an_automatic_size_is_still_drawn() {
        let filled = fill_form(
            &one_field([50, 700, 200, 730], 0, "/Helv 0 Tf 0 g", None),
            &[value("answer", "Amelie Durand-Chevalier de la Tour du Pin")],
        )
        .expect("filling should succeed");

        assert!(
            ink_in(&filled, 50, 110, 200, 145) > 20,
            "the answer is there"
        );
    }

    #[test]
    fn wrapping_keeps_every_word() {
        let lines = wrap(b"alpha beta gamma delta", 12.0, 60.0);
        let rejoined = lines.join(&b' ');
        assert_eq!(
            String::from_utf8_lossy(&rejoined),
            "alpha beta gamma delta",
            "wrapping must not drop or duplicate a word"
        );
        assert!(lines.len() > 1, "and it must actually have wrapped");
    }

    /// A word too long for the line is cut across lines rather than allowed to
    /// run past the edge — where the bounding box would clip it away.
    #[test]
    fn a_word_wider_than_the_line_is_broken_not_lost() {
        let word =
            "Donaudampfschiffahrtselektrizitaetenhauptbetriebswerkbauunterbeamtengesellschaft";
        let lines = wrap(word.as_bytes(), 12.0, 80.0);
        assert!(lines.len() > 1, "it has to break somewhere");
        assert_eq!(
            String::from_utf8_lossy(&lines.concat()),
            word,
            "and every letter survives the break"
        );
    }

    #[test]
    fn a_line_that_fits_is_left_whole() {
        assert_eq!(wrap(b"short", 12.0, 300.0), vec![b"short".to_vec()]);
    }

    /// The whole point of the module. Writing `/V` alone leaves most readers
    /// painting an empty box, so the test does not check the value — it checks
    /// that the page now has INK where the field is.
    #[test]
    fn a_filled_field_is_actually_visible() {
        let source = form_document();
        // The field's rect is y 700..730 from the bottom of an 842pt page, so
        // roughly y 112..142 from the top.
        assert_eq!(
            ink_in(&source, 50, 110, 350, 145),
            0,
            "the form starts empty"
        );

        let filled = fill_form(&source, &[value("fullName", "Amelie Durand")])
            .expect("filling should succeed");

        assert!(
            ink_in(&filled, 50, 110, 350, 145) > 50,
            "the value should be painted into the field"
        );
    }

    #[test]
    fn the_value_is_written_as_well_as_drawn() {
        let filled = fill_form(&form_document(), &[value("fullName", "Amélie Durand")]).unwrap();

        let fields = form_fields(&filled).unwrap();
        let name = fields.iter().find(|f| f.name == "fullName").unwrap();
        assert_eq!(name.value.as_deref(), Some("Amélie Durand"));
    }

    #[test]
    fn ticks_a_checkbox_with_the_state_the_document_accepts() {
        let filled = fill_form(&form_document(), &[value("accepted", "Yes")]).unwrap();

        let document = Document::load_mem(&filled).unwrap();
        let fields = form_fields(&filled).unwrap();
        let accepted = fields.iter().find(|f| f.name == "accepted").unwrap();
        assert_eq!(accepted.value.as_deref(), Some("Yes"));

        // /AS is what a reader paints from; without it the tick never shows.
        let has_appearance_state = document.objects.values().any(|object| {
            object
                .as_dict()
                .ok()
                .and_then(|dictionary| dictionary.get(b"AS").ok())
                .and_then(|state| state.as_name().ok())
                .is_some_and(|name| name == b"Yes")
        });
        assert!(has_appearance_state, "the widget should point at /Yes");
    }

    #[test]
    fn refuses_a_state_the_checkbox_does_not_have() {
        let error = fill_form(&form_document(), &[value("accepted", "On")])
            .expect_err("the document only accepts /Yes");

        assert!(error.contains("accepts"), "got: {error}");
    }

    #[test]
    fn accepts_a_choice_the_form_offers_and_refuses_the_rest() {
        assert!(fill_form(&form_document(), &[value("country", "FR")]).is_ok());

        let error = fill_form(&form_document(), &[value("country", "Belgique")])
            .expect_err("Belgique is not on offer");
        assert!(error.contains("accepts"), "got: {error}");
    }

    /// A name that does not exist must fail loudly: a filled document silently
    /// missing an answer is worse than an error.
    #[test]
    fn refuses_a_field_the_form_does_not_have() {
        let error = fill_form(&form_document(), &[value("telephone", "0600")])
            .expect_err("there is no telephone field");

        assert!(error.contains("no field named"), "got: {error}");
        // The message lists what IS there, so the caller can fix the name.
        assert!(error.contains("fullName"), "got: {error}");
    }

    #[test]
    fn refuses_a_read_only_field() {
        let error = fill_form(&form_document(), &[value("reference", "X")])
            .expect_err("the reference is read-only");

        assert!(error.contains("read-only"), "got: {error}");
    }

    #[test]
    fn enforces_the_maximum_length_the_field_declares() {
        assert!(fill_form(&form_document(), &[value("notes", "court")]).is_ok());

        let error = fill_form(
            &form_document(),
            &[value("notes", "beaucoup trop long pour ce champ")],
        )
        .expect_err("the field caps at 20 characters");
        assert!(error.contains("at most 20"), "got: {error}");
    }

    #[test]
    fn refuses_the_kinds_it_cannot_fill() {
        let error = fill_form(&form_document(), &[value("signature", "x")])
            .expect_err("a signature field cannot be filled this way");

        assert!(error.contains("signature"), "got: {error}");
    }

    #[test]
    fn refuses_text_a_standard_font_cannot_carry() {
        let error = fill_form(&form_document(), &[value("fullName", "договор")])
            .expect_err("Cyrillic has no WinAnsi byte");

        assert!(error.contains("WinAnsi"), "got: {error}");
    }

    #[test]
    fn fills_several_fields_at_once() {
        let filled = fill_form(
            &form_document(),
            &[
                value("fullName", "Amélie Durand"),
                value("accepted", "Yes"),
                value("country", "CH"),
            ],
        )
        .unwrap();

        let fields = form_fields(&filled).unwrap();
        let by_name = |name: &str| {
            fields
                .iter()
                .find(|f| f.name == name)
                .and_then(|f| f.value.clone())
        };
        assert_eq!(by_name("fullName").as_deref(), Some("Amélie Durand"));
        assert_eq!(by_name("accepted").as_deref(), Some("Yes"));
        assert_eq!(by_name("country").as_deref(), Some("CH"));
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(fill_form(b"not a PDF", &[value("x", "y")]).is_err());
    }

    #[test]
    fn parses_the_size_and_colour_out_of_a_default_appearance() {
        let appearance = parse_default_appearance("/Helv 14 Tf 1 0 0 rg");
        assert_eq!(appearance.size, 14.0);
        assert_eq!(appearance.color, [1.0, 0.0, 0.0]);

        // Size 0 means "fit the box", which needs glyph metrics; the default
        // is kept rather than a guess being painted.
        assert_eq!(parse_default_appearance("/Helv 0 Tf 0 g").size, 10.0);
    }
}
