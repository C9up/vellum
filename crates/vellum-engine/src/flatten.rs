//! Turning a filled form into ordinary page content.
//!
//! Flattening answers a need filling does not: a document nobody can edit any
//! more. Every widget's appearance stream is painted into the page it belongs
//! to, the widget annotations are dropped, and the catalog loses its
//! `/AcroForm` — after which the answers are ink like any other.
//!
//! The placement follows the algorithm of the specification (§12.5.5): the
//! appearance's `/BBox` is transformed by its `/Matrix`, and the box that
//! results is mapped onto the annotation's `/Rect`. Painting the stream at the
//! rectangle's corner instead would misplace every appearance whose form
//! matrix is not the identity.

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::fill::widgets_of;
use crate::form::fields_of;
use crate::page::{isolate_existing_contents, register_resources};

/// Bit 2 of an annotation's `/F`: the annotation is shown nowhere. Painting it
/// into the page would make visible what the document hid.
const HIDDEN: i64 = 1 << 1;

/// A rectangle with its corners in the order the arithmetic wants them.
#[derive(Debug, Clone, Copy)]
struct Rectangle {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rectangle {
    /// Read a PDF rectangle, whose corners may be given in any order.
    fn read(object: &Object) -> Option<Self> {
        let values = object.as_array().ok()?;
        let at = |index: usize| -> Option<f32> {
            values.get(index).and_then(|value| value.as_float().ok())
        };
        let (a, b, c, d) = (at(0)?, at(1)?, at(2)?, at(3)?);
        Some(Self {
            x0: a.min(c),
            y0: b.min(d),
            x1: a.max(c),
            y1: b.max(d),
        })
    }

    fn width(self) -> f32 {
        self.x1 - self.x0
    }

    fn height(self) -> f32 {
        self.y1 - self.y0
    }
}

/// The `/Matrix` of a form XObject, identity when it has none.
fn form_matrix(dictionary: &Dictionary) -> [f32; 6] {
    let mut matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let Ok(values) = dictionary
        .get(b"Matrix")
        .and_then(|object| object.as_array())
    else {
        return matrix;
    };
    if values.len() != 6 {
        return matrix;
    }
    for (slot, value) in matrix.iter_mut().zip(values) {
        let Ok(number) = value.as_float() else {
            return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        };
        *slot = number;
    }
    matrix
}

fn apply(matrix: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (
        matrix[0] * x + matrix[2] * y + matrix[4],
        matrix[1] * x + matrix[3] * y + matrix[5],
    )
}

/// The normal appearance a widget currently shows, if it ships one.
///
/// `/N` is either the stream itself or a dictionary of states, in which case
/// `/AS` says which one is on.
fn appearance_stream(document: &Document, widget: &Dictionary) -> Option<ObjectId> {
    let states = match widget.get(b"AP").ok()? {
        Object::Reference(id) => document.get_dictionary(*id).ok()?,
        Object::Dictionary(dictionary) => dictionary,
        _ => return None,
    }
    .get(b"N")
    .ok()?;

    let states = match states {
        Object::Reference(id) => match document.get_object(*id).ok()? {
            Object::Stream(_) => return Some(*id),
            Object::Dictionary(dictionary) => dictionary.clone(),
            _ => return None,
        },
        Object::Dictionary(dictionary) => dictionary.clone(),
        _ => return None,
    };

    let state = widget.get(b"AS").ok()?.as_name().ok()?;
    states.get(state).ok()?.as_reference().ok()
}

/// The transform that puts an appearance stream where its annotation sits.
fn placement(document: &Document, widget: &Dictionary, stream_id: ObjectId) -> Option<[f32; 6]> {
    let rect = Rectangle::read(widget.get(b"Rect").ok()?)?;
    let stream = document.get_object(stream_id).ok()?.as_stream().ok()?;
    let bbox = Rectangle::read(stream.dict.get(b"BBox").ok()?)?;
    let matrix = form_matrix(&stream.dict);

    let corners = [
        apply(matrix, bbox.x0, bbox.y0),
        apply(matrix, bbox.x1, bbox.y0),
        apply(matrix, bbox.x1, bbox.y1),
        apply(matrix, bbox.x0, bbox.y1),
    ];
    let (mut min_x, mut min_y) = corners[0];
    let (mut max_x, mut max_y) = corners[0];
    for (x, y) in corners {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    // A degenerate transformed box cannot be scaled onto anything; the
    // appearance is placed at its natural size rather than divided by zero.
    let ratio = |span: f32, of: f32| {
        if of.abs() > f32::EPSILON {
            span / of
        } else {
            1.0
        }
    };
    let sx = ratio(rect.width(), max_x - min_x);
    let sy = ratio(rect.height(), max_y - min_y);

    let placed = [sx, 0.0, 0.0, sy, rect.x0 - min_x * sx, rect.y0 - min_y * sy];
    placed
        .iter()
        .all(|value| value.is_finite())
        .then_some(placed)
}

/// The annotations of a page, whether the list is inline or referenced.
fn annotations_of(document: &Document, page_id: ObjectId) -> Vec<Object> {
    let annots = document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Annots").ok().cloned());

    match annots {
        Some(Object::Array(items)) => items,
        Some(Object::Reference(id)) => document
            .get_object(id)
            .ok()
            .and_then(|object| object.as_array().ok())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Drop the `/AcroForm` entry: with no widgets left, an interactive form
/// pointing at fields that draw nothing is worse than none at all.
fn remove_acroform(document: &mut Document) {
    let Ok(Object::Reference(catalog_id)) = document.trailer.get(b"Root").cloned() else {
        return;
    };
    if let Ok(catalog) = document
        .get_object_mut(catalog_id)
        .and_then(|object| object.as_dict_mut())
    {
        catalog.remove(b"AcroForm");
    }
}

/// Paint one page's widgets into its content and take them off its `/Annots`.
fn flatten_page(
    document: &mut Document,
    page_id: ObjectId,
    answered: &HashMap<ObjectId, String>,
) -> Result<(), String> {
    let mut kept: Vec<Object> = Vec::new();
    let mut painted: Vec<(String, ObjectId, [f32; 6])> = Vec::new();

    for annotation in annotations_of(document, page_id) {
        let id = annotation.as_reference().ok();
        let widget = match &annotation {
            Object::Reference(id) => document.get_dictionary(*id).ok().cloned(),
            Object::Dictionary(dictionary) => Some(dictionary.clone()),
            _ => None,
        };
        let Some(widget) = widget else {
            kept.push(annotation);
            continue;
        };

        let is_widget = widget
            .get(b"Subtype")
            .and_then(|subtype| subtype.as_name())
            .is_ok_and(|subtype| subtype == b"Widget");
        if !is_widget {
            kept.push(annotation);
            continue;
        }

        // From here the annotation leaves the page either way: a widget that
        // survived flattening would still be editable.
        let hidden = widget
            .get(b"F")
            .and_then(|flags| flags.as_i64())
            .is_ok_and(|flags| flags & HIDDEN != 0);
        if hidden {
            continue;
        }

        let placed = appearance_stream(document, &widget)
            .and_then(|stream_id| Some((stream_id, placement(document, &widget, stream_id)?)));

        let Some((stream_id, matrix)) = placed else {
            if let Some(name) = id.and_then(|id| answered.get(&id)) {
                return Err(format!(
                    "the field {name:?} holds a value but has no appearance stream to paint, \
                     so flattening it would drop the answer — fill the form first"
                ));
            }
            continue;
        };

        painted.push((format!("VellumFlat{}", painted.len()), stream_id, matrix));
    }

    if painted.is_empty() {
        return Ok(());
    }

    isolate_existing_contents(document, page_id)?;

    let entries: Vec<(String, ObjectId)> = painted
        .iter()
        .map(|(key, stream_id, _)| (key.clone(), *stream_id))
        .collect();
    register_resources(document, page_id, "XObject", &entries)?;

    let mut content: Vec<u8> = Vec::new();
    for (key, _, matrix) in &painted {
        content.extend_from_slice(
            format!(
                "q\n{} {} {} {} {} {} cm\n/{key} Do\nQ\n",
                matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]
            )
            .as_bytes(),
        );
    }
    document
        .add_page_contents(page_id, content)
        .map_err(|error| format!("cannot write page contents: {error}"))?;

    let page = document
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|error| format!("cannot update page: {error}"))?;
    if kept.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", Object::Array(kept));
    }
    Ok(())
}

/// Paint every form field into the page and remove the interactive layer.
///
/// The document keeps its look and loses its fields. A field that carries a
/// value but ships no appearance is an error rather than a silent erasure: the
/// answer would vanish from a document that still looks complete.
pub fn flatten_form(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut document =
        Document::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))?;

    // Which widgets carry an answer, so losing one can be reported by name.
    let mut answered: HashMap<ObjectId, String> = HashMap::new();
    for (field_id, field) in fields_of(&document) {
        let Some(value) = &field.value else { continue };
        if value.is_empty() || value == "Off" {
            continue;
        }
        for widget in widgets_of(&document, field_id) {
            answered.insert(widget, field.name.clone());
        }
    }

    for page_id in document.get_pages().into_values() {
        flatten_page(&mut document, page_id, &answered)?;
    }

    remove_acroform(&mut document);

    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use lopdf::{dictionary, Stream};

    use super::*;
    use crate::fill::tests::{form_document, ink_in};
    use crate::{fill_form, form_fields, FieldValue};

    fn filled() -> Vec<u8> {
        fill_form(
            &form_document(),
            &[FieldValue {
                name: "fullName".to_string(),
                value: "Amelie Durand".to_string(),
            }],
        )
        .expect("filling should succeed")
    }

    /// The point of flattening: the page looks the same afterwards. The field
    /// rect is y 700..730 from the bottom of an 842pt page, so roughly y
    /// 112..142 from the top.
    #[test]
    fn the_answer_survives_as_ink() {
        let before = ink_in(&filled(), 50, 110, 350, 145);
        assert!(before > 50, "the filled form starts with ink");

        let flat = flatten_form(&filled()).expect("flattening should succeed");
        let after = ink_in(&flat, 50, 110, 350, 145);
        assert!(
            after > 50,
            "the value should still be painted, got {after} dark pixels"
        );
    }

    #[test]
    fn the_interactive_layer_is_gone() {
        let flat = flatten_form(&filled()).expect("flattening should succeed");
        assert!(
            form_fields(&flat)
                .expect("the flattened document still parses")
                .is_empty(),
            "a flattened document has no fields left to fill"
        );

        let document = Document::load_mem(&flat).expect("the result parses");
        let page_id = *document
            .get_pages()
            .values()
            .next()
            .expect("the document has a page");
        assert!(
            document
                .get_dictionary(page_id)
                .expect("the page reads")
                .get(b"Annots")
                .is_err(),
            "the widgets are off the page"
        );
    }

    /// The placement has to honour the appearance's own `/Matrix`; painting at
    /// the rectangle's corner instead would put the ink in the wrong place.
    #[test]
    fn an_appearance_matrix_is_honoured() {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();

        // A 10x10 square drawn at the origin of the appearance's own space and
        // carried to (100, 100) by the form matrix alone.
        let appearance = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
                "Matrix" => vec![1.into(), 0.into(), 0.into(), 1.into(), 100.into(), 100.into()],
            },
            b"0 g\n0 0 10 10 re f\n".to_vec(),
        ));

        let widget = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Tx",
            "T" => lopdf::text_string("box"),
            "Rect" => vec![200.into(), 700.into(), 240.into(), 740.into()],
            "P" => page_id,
            "AP" => dictionary! { "N" => Object::Reference(appearance) },
        });

        document.objects.insert(
            page_id,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Annots" => vec![Object::Reference(widget)],
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
            "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(widget)] },
        });
        document.trailer.set("Root", catalog_id);

        let mut source = Vec::new();
        document.save_to(&mut source).expect("the fixture saves");

        let flat = flatten_form(&source).expect("flattening should succeed");
        // The rect is x 200..240, y 700..740 from the bottom, so y 102..142
        // from the top. The square is scaled to fill it exactly.
        assert!(
            ink_in(&flat, 200, 102, 240, 142) > 1000,
            "the square should fill the widget's rectangle"
        );
        // Its far corner is the discriminating part: an appearance placed at
        // the rectangle's origin without applying the matrix would leave this
        // area blank.
        assert!(
            ink_in(&flat, 230, 105, 238, 115) > 0,
            "the appearance should be scaled onto the whole rectangle"
        );
    }

    /// An answer that cannot be painted must be a failure, never a document
    /// that looks complete and says nothing.
    #[test]
    fn a_value_that_cannot_be_painted_is_refused() {
        let mut document = Document::load_mem(&form_document()).expect("the fixture parses");
        let notes = document
            .objects
            .iter_mut()
            .find_map(|(_, object)| {
                let dictionary = object.as_dict_mut().ok()?;
                let name = dictionary.get(b"T").ok()?.as_str().ok()?;
                (name == b"notes").then_some(dictionary)
            })
            .expect("the fixture has a notes field");
        notes.set("V", lopdf::text_string("nothing draws this"));

        let mut source = Vec::new();
        document.save_to(&mut source).expect("the fixture saves");

        let error = flatten_form(&source).expect_err("an unpaintable answer is refused");
        assert!(
            error.contains("notes"),
            "the message should name the field, got {error:?}"
        );
    }

    /// Flattening removes the form, not every annotation the document carries.
    #[test]
    fn other_annotations_are_left_alone() {
        let mut document = Document::load_mem(&filled()).expect("the filled document parses");
        let page_id = *document
            .get_pages()
            .values()
            .next()
            .expect("the document has a page");
        let link = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => vec![10.into(), 10.into(), 60.into(), 30.into()],
        });
        let mut annots = annotations_of(&document, page_id);
        annots.push(Object::Reference(link));
        document
            .get_object_mut(page_id)
            .and_then(|object| object.as_dict_mut())
            .expect("the page is a dictionary")
            .set("Annots", Object::Array(annots));

        let mut source = Vec::new();
        document.save_to(&mut source).expect("the fixture saves");

        let flat = flatten_form(&source).expect("flattening should succeed");
        let flattened = Document::load_mem(&flat).expect("the result parses");
        let page_id = *flattened
            .get_pages()
            .values()
            .next()
            .expect("the document has a page");
        let remaining = annotations_of(&flattened, page_id);
        assert_eq!(remaining.len(), 1, "only the link should remain");
    }

    /// Wrapping the page's own content is not free: get it wrong and the page
    /// loses what it already drew, or draws it under a transform it never
    /// asked for.
    #[test]
    fn what_the_page_already_drew_survives() {
        let stamped = crate::stamp_text(
            &filled(),
            "BROUILLON",
            &crate::TextStampOptions {
                x: 60.0,
                y: 300.0,
                size: 36.0,
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        let before = ink_in(&stamped, 55, 265, 400, 310);
        assert!(before > 50, "the stamp is on the page to begin with");

        let flat = flatten_form(&stamped).expect("flattening should succeed");
        let after = ink_in(&flat, 55, 265, 400, 310);
        assert!(
            after.abs_diff(before) * 20 < before,
            "the stamp should be untouched, went from {before} to {after} dark pixels"
        );
        assert!(
            ink_in(&flat, 50, 110, 350, 145) > 50,
            "and the flattened answer should be there too"
        );
    }

    /// A page may leave the graphics state transformed: `cm` outside any
    /// `q`/`Q` pair is legal and never restored. Content appended after it
    /// would inherit that transform and land somewhere else entirely.
    #[test]
    fn an_open_transform_does_not_move_the_answer() {
        let mut document = Document::load_mem(&filled()).expect("the filled document parses");
        let page_id = *document
            .get_pages()
            .values()
            .next()
            .expect("the document has a page");
        document
            .add_page_contents(page_id, b"2 0 0 2 0 0 cm\n".to_vec())
            .expect("the fixture takes contents");

        let mut source = Vec::new();
        document.save_to(&mut source).expect("the fixture saves");

        let flat = flatten_form(&source).expect("flattening should succeed");
        assert!(
            ink_in(&flat, 50, 110, 350, 145) > 50,
            "the answer belongs where its rectangle is, not where the page's \
             leftover transform would put it"
        );
    }

    #[test]
    fn a_hidden_widget_is_not_painted() {
        let mut document = Document::load_mem(&filled()).expect("the filled document parses");
        let widget = document
            .objects
            .iter_mut()
            .find_map(|(_, object)| {
                let dictionary = object.as_dict_mut().ok()?;
                let name = dictionary.get(b"T").ok()?.as_str().ok()?;
                (name == b"fullName").then_some(dictionary)
            })
            .expect("the fixture has a fullName field");
        widget.set("F", HIDDEN);

        let mut source = Vec::new();
        document.save_to(&mut source).expect("the fixture saves");

        let flat = flatten_form(&source).expect("flattening should succeed");
        assert_eq!(
            ink_in(&flat, 50, 110, 350, 145),
            0,
            "what the document hid stays hidden"
        );
    }
}
