//! Operations on a page of a document that already exists.
//!
//! Everything here is shared by the modules that draw onto someone else's
//! document rather than authoring a new one: stamping, and flattening a form.
//! Their hazards are the same, so the answers to them live in one place.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// Balance whatever the page already draws before appending to it.
///
/// A `cm` outside any `q`/`Q` pair is legal and never restored, so a page is
/// free to leave the graphics state transformed for everything that follows.
/// Content appended after it would inherit that transform and land somewhere
/// else entirely; wrapping the existing streams hands us the identity matrix
/// our coordinates were computed against.
pub(crate) fn isolate_existing_contents(
    document: &mut Document,
    page_id: ObjectId,
) -> Result<(), String> {
    let existing = document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Contents").ok().cloned());

    let mut streams: Vec<Object> = match existing {
        Some(Object::Array(items)) => items,
        Some(Object::Reference(id)) => vec![Object::Reference(id)],
        _ => return Ok(()),
    };
    if streams.is_empty() {
        return Ok(());
    }

    let open = document.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = document.add_object(Stream::new(Dictionary::new(), b"Q\n".to_vec()));
    streams.insert(0, Object::Reference(open));
    streams.push(Object::Reference(close));

    let page = document
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|error| format!("cannot update page: {error}"))?;
    page.set("Contents", Object::Array(streams));
    Ok(())
}

/// Name objects in one category of a page's `Resources` — `Font`, `XObject`,
/// `ExtGState` — so the page's content stream can refer to them.
pub(crate) fn register_resources(
    document: &mut Document,
    page_id: ObjectId,
    category: &str,
    entries: &[(String, ObjectId)],
) -> Result<(), String> {
    // Resources may be an inline dictionary or a reference to one; both are
    // legal, and the page may carry none at all.
    let existing = document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Resources").ok().cloned());

    let mut resources = match existing {
        Some(Object::Reference(id)) => document
            .get_dictionary(id)
            .cloned()
            .map_err(|error| format!("cannot read page resources: {error}"))?,
        Some(Object::Dictionary(dictionary)) => dictionary,
        _ => Dictionary::new(),
    };

    let mut group = match resources.get(category.as_bytes()) {
        Ok(Object::Reference(id)) => document.get_dictionary(*id).cloned().unwrap_or_default(),
        Ok(Object::Dictionary(dictionary)) => dictionary.clone(),
        _ => Dictionary::new(),
    };
    for (key, id) in entries {
        group.set(key.as_str(), Object::Reference(*id));
    }
    resources.set(category, Object::Dictionary(group));

    // Written back inline so the page owns them, rather than mutating a
    // resource dictionary that other pages may share.
    let page = document
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|error| format!("cannot update page: {error}"))?;
    page.set("Resources", Object::Dictionary(resources));
    Ok(())
}

/// The height of a page, needed to turn a top-down y into PDF's bottom-up one.
pub(crate) fn page_height(document: &Document, page_id: ObjectId) -> f32 {
    document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"MediaBox").ok())
        .and_then(|media_box| media_box.as_array().ok())
        .and_then(|values| {
            let bottom = values.get(1)?.as_float().ok()?;
            let top = values.get(3)?.as_float().ok()?;
            Some((top - bottom).abs())
        })
        .unwrap_or(841.89)
}
