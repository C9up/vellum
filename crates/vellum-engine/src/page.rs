//! Operations on a page of a document that already exists.
//!
//! Everything here is shared by the modules that draw onto someone else's
//! document rather than authoring a new one: stamping, and flattening a form.
//! Their hazards are the same, so the answers to them live in one place.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// The pages of a document in order, each page object once.
///
/// lopdf's page map yields a page once per `/Kids` entry that leads to it, so
/// a document listing one page object ten thousand times counted as ten
/// thousand pages from a few kilobytes, and every per-page operation did its
/// work that many times over — on the same page. A page tree is a tree: a
/// page reached twice is refused rather than counted twice.
pub(crate) fn page_ids(document: &Document) -> Result<Vec<ObjectId>, String> {
    let mut seen = HashSet::new();
    let mut pages = Vec::new();
    for page_id in document.page_iter() {
        if !seen.insert(page_id) {
            return Err(format!(
                "cannot read PDF: the page tree lists page object {} {} more than once",
                page_id.0, page_id.1
            ));
        }
        pages.push(page_id);
    }
    Ok(pages)
}

/// The most values one edit may copy out of a document across its pages:
/// every array element, dictionary value and stream counted once. As many as
/// parsing may build, and about as much memory: a few hundred megabytes.
pub(crate) const MAX_COPIED_VALUES: usize = 4_000_000;

/// The most bytes — strings, names, stream data — one edit may copy out of a
/// document across its pages: 256 MiB.
pub(crate) const MAX_COPIED_BYTES: usize = 256 * 1024 * 1024;

/// The bytes a split may copy into its parts: 1 GiB. Each part carries its
/// page's own images and fonts, so a large scan legitimately comes out as
/// large as it went in.
pub(crate) const MAX_SPLIT_BYTES: usize = 1024 * 1024 * 1024;

/// What one edit may still copy out of the document it edits.
///
/// Pages share what they draw with: one `/Font` group, one `/Resources` on
/// the page tree, one `/Annots` array, one catalog every part of a split
/// carries. Copied once per page, a group of a million entries shared by a
/// million pages is a trillion values from a few megabytes. Every copy made
/// per page is charged here, by what it holds rather than by the one object
/// it is, and the edit is refused once the allowance is spent.
pub(crate) struct Copies {
    values: usize,
    bytes: usize,
    /// The byte ceiling, for the refusal to name.
    max_bytes: usize,
    /// What the refusal suggests instead, if anything.
    hint: &'static str,
}

impl Copies {
    /// The allowance of one edit: [`MAX_COPIED_VALUES`] and
    /// [`MAX_COPIED_BYTES`].
    pub(crate) fn new() -> Self {
        Self {
            values: MAX_COPIED_VALUES,
            bytes: MAX_COPIED_BYTES,
            max_bytes: MAX_COPIED_BYTES,
            hint: "",
        }
    }

    /// The allowance of a split, across all its parts.
    pub(crate) fn splitting() -> Self {
        Self {
            values: MAX_COPIED_VALUES,
            bytes: MAX_SPLIT_BYTES,
            max_bytes: MAX_SPLIT_BYTES,
            hint: " — select the pages you need instead",
        }
    }

    /// No limit: for a copy that takes each object of the document once,
    /// which the parse budget already bounds.
    pub(crate) fn unbounded() -> Self {
        Self {
            values: usize::MAX,
            bytes: usize::MAX,
            max_bytes: usize::MAX,
            hint: "",
        }
    }

    /// Charge a copy of `object`, everything it holds included.
    pub(crate) fn charge(&mut self, object: &Object) -> Result<(), String> {
        let (values, bytes) = weight(object);
        self.spend(values, bytes)
    }

    /// Charge a copy of `dictionary`, everything it holds included.
    pub(crate) fn charge_dictionary(&mut self, dictionary: &Dictionary) -> Result<(), String> {
        let (values, bytes) = dictionary_weight(dictionary);
        self.spend(values, bytes)
    }

    fn spend(&mut self, values: usize, bytes: usize) -> Result<(), String> {
        if values > self.values || bytes > self.bytes {
            return Err(format!(
                "editing this document would copy more than {MAX_COPIED_VALUES} values or {} MiB of it across its pages{}",
                self.max_bytes / (1024 * 1024),
                self.hint
            ));
        }
        self.values -= values;
        self.bytes -= bytes;
        Ok(())
    }
}

/// How many values `object` holds, itself included, and how many bytes of
/// strings, names and stream data.
fn weight(object: &Object) -> (usize, usize) {
    match object {
        Object::Name(name) => (1, name.len()),
        Object::String(string, _) => (1, string.len()),
        Object::Array(items) => items.iter().fold((1, 0), |(values, bytes), item| {
            let (more, larger) = weight(item);
            (values.saturating_add(more), bytes.saturating_add(larger))
        }),
        Object::Dictionary(dictionary) => dictionary_weight(dictionary),
        Object::Stream(stream) => {
            let (values, bytes) = dictionary_weight(&stream.dict);
            (values, bytes.saturating_add(stream.content.len()))
        }
        _ => (1, 0),
    }
}

fn dictionary_weight(dictionary: &Dictionary) -> (usize, usize) {
    dictionary
        .iter()
        .fold((1, 0), |(values, bytes), (key, value)| {
            let (more, larger) = weight(value);
            (
                values.saturating_add(more),
                bytes.saturating_add(key.len()).saturating_add(larger),
            )
        })
}

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
    copies: &mut Copies,
) -> Result<(), String> {
    let existing = match document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Contents").ok())
    {
        Some(contents) => {
            copies.charge(contents)?;
            Some(contents.clone())
        }
        None => None,
    };

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
/// `ExtGState` — so the page's content stream can refer to them, and answer
/// the names given, in order.
///
/// A name already taken on the page is never reused: `base`, then `base1`,
/// `base2`… The content already on the page refers to the objects it was
/// written against by name, so overwriting a name repaints that content with
/// whatever was registered last — the second font, the second image.
pub(crate) fn register_resources(
    document: &mut Document,
    page_id: ObjectId,
    category: &str,
    entries: &[(&str, ObjectId)],
    copies: &mut Copies,
) -> Result<Vec<String>, String> {
    // Resources may be an inline dictionary or a reference to one; both are
    // legal, and the page may carry none at all.
    let existing = document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Resources").ok());

    // Copied for this page: charged, since every page may share them.
    let mut resources = match existing {
        Some(Object::Reference(id)) => {
            let shared = document
                .get_dictionary(*id)
                .map_err(|error| format!("cannot read page resources: {error}"))?;
            copies.charge_dictionary(shared)?;
            shared.clone()
        }
        Some(Object::Dictionary(dictionary)) => {
            copies.charge_dictionary(dictionary)?;
            dictionary.clone()
        }
        _ => Dictionary::new(),
    };

    let mut group = match resources.get(category.as_bytes()) {
        Ok(Object::Reference(id)) => match document.get_dictionary(*id) {
            Ok(shared) => {
                copies.charge_dictionary(shared)?;
                shared.clone()
            }
            Err(_) => Dictionary::new(),
        },
        Ok(Object::Dictionary(dictionary)) => dictionary.clone(),
        _ => Dictionary::new(),
    };
    let mut names = Vec::with_capacity(entries.len());
    for (base, id) in entries {
        let name = free_name(&group, base);
        group.set(name.as_str(), Object::Reference(*id));
        names.push(name);
    }
    resources.set(category, Object::Dictionary(group));

    // Written back inline so the page owns them, rather than mutating a
    // resource dictionary that other pages may share.
    let page = document
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|error| format!("cannot update page: {error}"))?;
    page.set("Resources", Object::Dictionary(resources));
    Ok(names)
}

/// `base` if no entry of `group` has that name, else the first free
/// `base<n>`.
fn free_name(group: &Dictionary, base: &str) -> String {
    if !group.has(base.as_bytes()) {
        return base.to_string();
    }
    let mut n: u32 = 1;
    loop {
        let candidate = format!("{base}{n}");
        if !group.has(candidate.as_bytes()) {
            return candidate;
        }
        n += 1;
    }
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
