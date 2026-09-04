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
use crate::stamp_text::{escape_pdf_literal, to_win_ansi};

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
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            size: 10.0,
            color: [0.0, 0.0, 0.0],
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
                    // Size 0 means "fit the box", which needs glyph metrics to
                    // do properly; the default is kept instead of guessing.
                    if size > 0.0 {
                        appearance.size = size;
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

/// Build the appearance stream that paints `text` inside a widget.
fn text_appearance(
    width: f32,
    height: f32,
    text: &[u8],
    appearance: Appearance,
    multiline: bool,
) -> Stream {
    const PADDING: f32 = 2.0;
    let [red, green, blue] = appearance.color;

    let mut content: Vec<u8> = Vec::new();
    // /Tx BMC ... EMC marks this as a form field's appearance, which is what
    // tells a reader it may replace it when the value changes.
    content.extend_from_slice(b"/Tx BMC\nq\nBT\n");
    content.extend_from_slice(
        format!(
            "/{FONT_KEY} {size} Tf\n{red} {green} {blue} rg\n",
            size = appearance.size,
        )
        .as_bytes(),
    );

    // A single-line field centres its text vertically; a multiline one starts
    // at the top. No word wrapping is attempted — that needs glyph metrics —
    // so only the line breaks the caller wrote are honoured.
    let first_baseline = if multiline {
        height - PADDING - appearance.size
    } else {
        (height - appearance.size) / 2.0 + appearance.size * 0.2
    };
    content.extend_from_slice(
        format!(
            "{leading} TL\n1 0 0 1 {PADDING} {first_baseline} Tm\n",
            leading = appearance.size * 1.2
        )
        .as_bytes(),
    );

    for (index, line) in text.split(|byte| *byte == b'\n').enumerate() {
        if index > 0 {
            content.extend_from_slice(b"T*\n");
        }
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
                    let stream =
                        text_appearance(width, height, &encoded, appearance, field.multiline);
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
