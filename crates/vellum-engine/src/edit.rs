//! Operations on documents that already exist: merge, select, split, rotate.
//!
//! All of these move pages between page trees, which is where PDF hides a
//! trap: `Resources`, `MediaBox`, `CropBox` and `Rotate` may live on a parent
//! `Pages` node and be INHERITED by the page (PDF 32000-1 §7.7.3.4). Re-parent
//! such a page and it silently loses its size and its fonts. Every operation
//! here materialises the inherited attributes onto the page first.
//!
//! Every one of them also costs what the document weighs, once. lopdf's own
//! `delete_pages`, `prune_objects` and `renumber_objects` each walk the whole
//! document, and that walk is itself quadratic in the object count; called
//! once per page, a 75 KB file of a thousand pages held a worker for a minute.
//! A document is cut apart here by copying what the kept pages reach, in one
//! walk.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use crate::page::{page_ids, Copies};

/// Attributes a page inherits from its ancestors in the page tree.
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// How far up a page's `/Parent` chain inheritance is looked for: lopdf reads
/// a page tree 256 levels deep at most, so a page it found has no more real
/// ancestors than that, and a longer chain is not a page tree.
const MAX_ANCESTORS: usize = 257;

fn load(bytes: &[u8]) -> Result<Document, String> {
    crate::parse::parse(bytes)
}

fn save(mut document: Document) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out)
}

/// The page at zero-based `index`, or the error that names it in human
/// numbering.
fn page_at(pages: &[ObjectId], index: u32) -> Result<ObjectId, String> {
    usize::try_from(index)
        .ok()
        .and_then(|index| pages.get(index).copied())
        .ok_or_else(|| {
            // Widened before adding one: u32::MAX + 1 wraps to 0 in a release
            // build, and "page 0 does not exist" names nothing.
            format!(
                "page {} does not exist — the document has {}",
                u64::from(index) + 1,
                pages.len()
            )
        })
}

/// Walk up from each page and copy any inherited attribute onto the page.
///
/// Without this, a page whose `MediaBox` lives on its parent comes out of a
/// merge or a split with no size at all — which readers fall back to Letter
/// for, quietly resizing a French A4 document.
///
/// What is copied is charged to `copies`: an inline `/Resources` on the page
/// tree is copied onto every page below it.
pub(crate) fn flatten_inheritance(
    document: &mut Document,
    pages: &[ObjectId],
    copies: &mut Copies,
) -> Result<(), String> {
    for &page_id in pages {
        let mut inherited: Vec<(Vec<u8>, Object)> = Vec::new();

        {
            let Ok(page) = document.get_dictionary(page_id) else {
                continue;
            };
            let mut missing: Vec<&[u8]> = INHERITABLE
                .iter()
                .copied()
                .filter(|key| page.get(key).is_err())
                .collect();
            if missing.is_empty() {
                continue;
            }

            // Guarded against a cycle in the parent chain, which a crafted
            // document can carry and which would otherwise spin forever, and
            // against a chain longer than any page tree, which would make the
            // walk cost pages x chain.
            let mut seen = HashSet::new();
            let mut next = page.get(b"Parent").and_then(Object::as_reference).ok();
            while let Some(parent_id) = next {
                if seen.len() >= MAX_ANCESTORS || !seen.insert(parent_id) {
                    break;
                }
                let Ok(parent) = document.get_dictionary(parent_id) else {
                    break;
                };
                let mut found = Vec::new();
                missing.retain(|key| match parent.get(key) {
                    Ok(value) => {
                        found.push((key.to_vec(), value));
                        false
                    }
                    Err(_) => true,
                });
                for (key, value) in found {
                    copies.charge(value)?;
                    inherited.push((key, value.clone()));
                }
                if missing.is_empty() {
                    break;
                }
                next = parent.get(b"Parent").and_then(Object::as_reference).ok();
            }
        }

        if let Ok(page) = document
            .get_object_mut(page_id)
            .and_then(|object| object.as_dict_mut())
        {
            for (key, value) in inherited {
                page.set(key, value);
            }
        }
    }
    Ok(())
}

/// A document's pages and the nodes of the tree that holds them.
struct PageTree {
    pages: Vec<ObjectId>,
    members: HashSet<ObjectId>,
}

impl PageTree {
    /// Read the page tree, each page once, and its intermediate nodes: every
    /// object typed `/Pages`.
    fn of(document: &Document) -> Result<Self, String> {
        let pages = page_ids(document)?;
        let mut members: HashSet<ObjectId> = pages.iter().copied().collect();
        members.extend(
            document
                .objects
                .iter()
                .filter(|(_, object)| has_type(object, b"Pages"))
                .map(|(id, _)| *id),
        );
        Ok(Self { pages, members })
    }
}

fn has_type(object: &Object, name: &[u8]) -> bool {
    let dictionary = match object {
        Object::Dictionary(dictionary) => dictionary,
        Object::Stream(stream) => &stream.dict,
        _ => return false,
    };
    dictionary
        .get(b"Type")
        .and_then(Object::as_name)
        .is_ok_and(|kind| kind == name)
}

/// Which references a copy keeps, rewrites or drops.
enum Reach {
    /// Copied, and followed.
    Follow,
    /// Removed, as `delete_pages` removes a reference to a page it deletes:
    /// the array entry or the dictionary key goes.
    Cut,
    /// Points at nothing: a reader takes it as `null`, and so is it written.
    Missing,
}

/// Whether `object` is a reference `reach` cuts.
fn is_cut(object: &Object, reach: &impl Fn(ObjectId) -> Reach) -> bool {
    match object {
        Object::Reference(id) => matches!(reach(*id), Reach::Cut),
        _ => false,
    }
}

/// Remove every reference `reach` cuts from `object`, write the missing ones
/// as `null`, and queue the ones it follows.
fn strip(object: &mut Object, reach: &impl Fn(ObjectId) -> Reach, found: &mut Vec<ObjectId>) {
    match object {
        Object::Array(items) => {
            items.retain(|item| !is_cut(item, reach));
            for item in items.iter_mut() {
                strip(item, reach, found);
            }
        }
        Object::Dictionary(dictionary) => strip_dictionary(dictionary, reach, found),
        Object::Stream(stream) => strip_dictionary(&mut stream.dict, reach, found),
        Object::Reference(id) => match reach(*id) {
            Reach::Follow => found.push(*id),
            Reach::Missing => *object = Object::Null,
            // Removed by the array or dictionary that holds it.
            Reach::Cut => {}
        },
        _ => {}
    }
}

fn strip_dictionary(
    dictionary: &mut Dictionary,
    reach: &impl Fn(ObjectId) -> Reach,
    found: &mut Vec<ObjectId>,
) {
    let cut: Vec<Vec<u8>> = dictionary
        .iter()
        .filter(|(_, value)| is_cut(value, reach))
        .map(|(key, _)| key.clone())
        .collect();
    for key in cut {
        dictionary.remove(&key);
    }
    for (_, value) in dictionary.iter_mut() {
        strip(value, reach, found);
    }
}

/// Copy `seeds` and everything they reach into `into`, without what `reach`
/// cuts. `prepare` sees each object before its references are followed.
fn copy_reachable(
    source: &Document,
    seeds: Vec<ObjectId>,
    reach: &impl Fn(ObjectId) -> Reach,
    prepare: &impl Fn(ObjectId, &mut Object),
    copies: &mut Copies,
    into: &mut BTreeMap<ObjectId, Object>,
) -> Result<(), String> {
    let mut queue: VecDeque<ObjectId> = seeds.into();
    let mut found = Vec::new();
    while let Some(id) = queue.pop_front() {
        if into.contains_key(&id) {
            continue;
        }
        let Some(original) = source.objects.get(&id) else {
            continue;
        };
        copies.charge(original)?;
        let mut object = original.clone();
        prepare(id, &mut object);
        strip(&mut object, reach, &mut found);
        queue.extend(found.drain(..));
        into.insert(id, object);
    }
    Ok(())
}

/// What a part of a document carries besides its pages.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// The catalog and what it reaches — the outline, the form, the names —
    /// and the trailer's `/Info`: everything the document held that does not
    /// lead to another page, as `delete_pages` followed by `prune_objects`
    /// left it.
    Catalog,
    /// Only the pages and what they reach, for merging under a new catalog.
    PagesOnly,
}

/// The objects of `document` that `kept` reach, under a single flat page tree
/// holding `kept` in that order, with every reference to another page or to
/// the old page tree removed.
fn part_of(
    document: &Document,
    tree: &PageTree,
    kept: &[ObjectId],
    keep: Keep,
    copies: &mut Copies,
) -> Result<Document, String> {
    let root = document
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| "no catalog found".to_string())?;
    let kept_set: HashSet<ObjectId> = kept.iter().copied().collect();
    let reach = |id: ObjectId| {
        let Some(object) = document.objects.get(&id) else {
            return Reach::Missing;
        };
        if kept_set.contains(&id) {
            return Reach::Follow;
        }
        let dropped = tree.members.contains(&id)
            || (keep == Keep::PagesOnly
                && (id == root
                    || has_type(object, b"Catalog")
                    || has_type(object, b"Outlines")
                    || has_type(object, b"Outline")));
        if dropped {
            Reach::Cut
        } else {
            Reach::Follow
        }
    };
    let prepare = |id: ObjectId, object: &mut Object| {
        if let Object::Dictionary(dictionary) = object {
            // The page is re-parented and the catalog given the new tree;
            // neither old link is followed.
            if kept_set.contains(&id) {
                dictionary.remove(b"Parent");
            } else if id == root {
                dictionary.remove(b"Pages");
            }
        }
    };

    let pages_id = (
        document
            .max_id
            .checked_add(1)
            .ok_or_else(|| "the document numbers its objects past the end".to_string())?,
        0,
    );
    let mut part = Document::with_version(document.version.clone());
    let mut seeds: Vec<ObjectId> = kept.to_vec();
    if keep == Keep::Catalog {
        part.trailer = document.trailer.clone();
        strip_dictionary(&mut part.trailer, &reach, &mut seeds);
    }
    copy_reachable(document, seeds, &reach, &prepare, copies, &mut part.objects)?;

    for page_id in kept {
        if let Some(Object::Dictionary(page)) = part.objects.get_mut(page_id) {
            page.set("Parent", pages_id);
        }
    }
    let kids: Vec<Object> = kept.iter().copied().map(Object::Reference).collect();
    part.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => i64::try_from(kids.len()).unwrap_or(i64::MAX),
            "Kids" => kids,
        }),
    );
    if keep == Keep::Catalog {
        match part.objects.get_mut(&root) {
            Some(Object::Dictionary(catalog)) => catalog.set("Pages", pages_id),
            _ => return Err("no catalog found".to_string()),
        }
    }
    part.max_id = pages_id.0;
    Ok(part)
}

/// Objects under their new numbers, and the old number of each mapped to its
/// new one.
type Renumbered = (BTreeMap<ObjectId, Object>, HashMap<ObjectId, ObjectId>);

/// Renumber `objects` from `next` on, in their order, and rewrite every
/// reference between them: one walk, where lopdf's `renumber_objects` walks
/// the document once per object.
fn renumber(objects: BTreeMap<ObjectId, Object>, next: &mut u32) -> Result<Renumbered, String> {
    let mut map = HashMap::with_capacity(objects.len());
    for id in objects.keys() {
        map.insert(*id, (*next, 0));
        *next = next
            .checked_add(1)
            .ok_or_else(|| "the merged document has too many objects".to_string())?;
    }
    let renumbered = objects
        .into_iter()
        .filter_map(|(id, mut object)| {
            rewrite_references(&mut object, &map);
            map.get(&id).map(|new_id| (*new_id, object))
        })
        .collect();
    Ok((renumbered, map))
}

fn rewrite_references(object: &mut Object, map: &HashMap<ObjectId, ObjectId>) {
    match object {
        Object::Array(items) => {
            for item in items {
                rewrite_references(item, map);
            }
        }
        Object::Dictionary(dictionary) => {
            for (_, value) in dictionary.iter_mut() {
                rewrite_references(value, map);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                rewrite_references(value, map);
            }
        }
        Object::Reference(id) => {
            // Every reference left in a part points into it: `part_of` wrote
            // the others as null or removed them.
            if let Some(new_id) = map.get(id) {
                *id = *new_id;
            }
        }
        _ => {}
    }
}

/// Join documents end to end, in the order given.
pub fn merge(documents: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if documents.is_empty() {
        return Err("merging needs at least one document".to_string());
    }

    // 1 and 2 are the new catalog and page tree.
    let catalog_id = (1, 0);
    let pages_id = (2, 0);
    let mut next = 3;
    let mut merged = Document::with_version("1.7");
    let mut kids = Vec::new();

    for (index, bytes) in documents.iter().enumerate() {
        let failed = |error: String| format!("document {}: {error}", index + 1);
        let mut document = load(bytes).map_err(failed)?;
        let tree = PageTree::of(&document).map_err(failed)?;
        flatten_inheritance(&mut document, &tree.pages, &mut Copies::new()).map_err(failed)?;

        // Outlines refer to the page trees being dismantled, and every
        // catalog but the new one goes, so only the pages and what they
        // reach are carried over.
        let part = part_of(
            &document,
            &tree,
            &tree.pages,
            Keep::PagesOnly,
            &mut Copies::unbounded(),
        )
        .map_err(failed)?;
        let mut objects = part.objects;
        objects.retain(|id, _| id.0 <= document.max_id);
        let (objects, map) = renumber(objects, &mut next).map_err(failed)?;
        for page_id in &tree.pages {
            if let Some(new_id) = map.get(page_id) {
                kids.push(Object::Reference(*new_id));
            }
        }
        merged.objects.extend(objects);
    }

    if kids.is_empty() {
        return Err("none of the documents has a page".to_string());
    }
    for kid in &kids {
        if let Ok(id) = kid.as_reference() {
            if let Some(Object::Dictionary(page)) = merged.objects.get_mut(&id) {
                page.set("Parent", pages_id);
            }
        }
    }
    merged.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => i64::try_from(kids.len()).unwrap_or(i64::MAX),
            "Kids" => kids,
        }),
    );
    merged.objects.insert(
        catalog_id,
        Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        }),
    );
    merged.trailer.set("Root", catalog_id);
    merged.max_id = next - 1;

    save(merged)
}

/// Keep only `pages`, addressed from ZERO, in the order the document has them.
///
/// Zero-based like every other page argument in this engine, even though
/// lopdf's own page map is one-based — mixing the two conventions inside one
/// engine is how an off-by-one gets built in. The conversion happens here and
/// nowhere else.
pub fn select_pages(bytes: &[u8], pages: &[u32]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("selecting needs at least one page".to_string());
    }

    let mut document = load(bytes)?;
    let tree = PageTree::of(&document)?;

    let mut wanted = BTreeSet::new();
    for &page in pages {
        page_at(&tree.pages, page)?;
        wanted.insert(page);
    }
    let kept: Vec<ObjectId> = tree
        .pages
        .iter()
        .enumerate()
        .filter(|(index, _)| u32::try_from(*index).is_ok_and(|index| wanted.contains(&index)))
        .map(|(_, page_id)| *page_id)
        .collect();
    // Only the pages kept: the others are not part of what comes out.
    flatten_inheritance(&mut document, &kept, &mut Copies::new())?;

    save(part_of(
        &document,
        &tree,
        &kept,
        Keep::Catalog,
        &mut Copies::unbounded(),
    )?)
}

/// One single-page document per page, in document order.
///
/// Every part carries what its page reaches and what the catalog reaches —
/// fonts, images, the outline, the form — so the parts together can weigh
/// the document times its page count. They share one [`Copies`] allowance,
/// and past it the split is refused rather than built.
pub fn split(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut document = load(bytes)?;
    let tree = PageTree::of(&document)?;
    let mut copies = Copies::splitting();
    flatten_inheritance(&mut document, &tree.pages, &mut copies)?;

    tree.pages
        .iter()
        .map(|page_id| {
            part_of(&document, &tree, &[*page_id], Keep::Catalog, &mut copies).and_then(save)
        })
        .collect()
}

/// Rotate pages clockwise by `degrees`, a multiple of 90.
///
/// `pages` selects which, addressed from ZERO; `None` rotates all of them. The
/// rotation is added to whatever the page already carries, because a document
/// can arrive with pages already rotated.
pub fn rotate(bytes: &[u8], degrees: i64, pages: Option<&[u32]>) -> Result<Vec<u8>, String> {
    if degrees % 90 != 0 {
        return Err(format!("rotation must be a multiple of 90, got {degrees}"));
    }

    let mut document = load(bytes)?;
    let all = page_ids(&document)?;

    let targets: Vec<ObjectId> = match pages {
        Some(wanted) => {
            let mut targets = Vec::with_capacity(wanted.len());
            for &page in wanted {
                targets.push(page_at(&all, page)?);
            }
            targets
        }
        None => all,
    };

    for page_id in targets {
        let Ok(page) = document
            .get_object_mut(page_id)
            .and_then(|object| object.as_dict_mut())
        else {
            continue;
        };
        let current = page.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0);
        // Normalised into 0..360: Rust's `%` keeps the sign, and a negative
        // /Rotate is legal but needlessly surprising to read back.
        page.set("Rotate", (current + degrees).rem_euclid(360));
    }

    save(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_blank, inspect, page_dimensions};

    const A4: (f32, f32) = (595.28, 841.89);
    const A5: (f32, f32) = (419.53, 595.28);

    /// A document whose `MediaBox` lives ONLY on the parent `Pages` node, so
    /// each page inherits it. Producers do this routinely, and it is what
    /// breaks a naive merge or split.
    fn document_with_inherited_media_box(page_count: usize) -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();

        let page_ids: Vec<Object> = (0..page_count)
            .map(|_| {
                // Note what is NOT here: no MediaBox, no Resources.
                Object::Reference(document.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                }))
            })
            .collect();

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => page_count as i64,
                "Kids" => page_ids,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Resources" => dictionary! {},
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn merges_documents_end_to_end() {
        let merged = merge(&[
            create_blank(&[A4, A4]).unwrap(),
            create_blank(&[A4, A4, A4]).unwrap(),
        ])
        .expect("merging should succeed");

        // Read back by BOTH parsers: lopdf counts the pages, hayro renders
        // them. A structurally broken merge usually satisfies one and not the
        // other.
        assert_eq!(inspect(&merged).unwrap().page_count, 5);
        assert_eq!(page_dimensions(&merged).unwrap().len(), 5);
    }

    #[test]
    fn merging_keeps_each_page_its_own_size() {
        let merged = merge(&[create_blank(&[A4]).unwrap(), create_blank(&[A5]).unwrap()])
            .expect("merging should succeed");

        let sizes = page_dimensions(&merged).unwrap();
        assert_eq!(sizes.len(), 2);
        assert!(
            (sizes[0].width - A4.0).abs() < 1.0,
            "got {}",
            sizes[0].width
        );
        assert!(
            (sizes[1].width - A5.0).abs() < 1.0,
            "got {}",
            sizes[1].width
        );
    }

    #[test]
    fn merging_a_single_document_keeps_its_pages() {
        let merged = merge(&[create_blank(&[A4, A4]).unwrap()]).unwrap();

        assert_eq!(inspect(&merged).unwrap().page_count, 2);
    }

    /// The reason `flatten_inheritance` exists. Merging re-parents every page
    /// under a new tree; without materialising the inherited `MediaBox` first,
    /// these pages come out sizeless and readers fall back to Letter.
    #[test]
    fn merging_preserves_an_inherited_media_box() {
        let merged = merge(&[
            document_with_inherited_media_box(1),
            document_with_inherited_media_box(1),
        ])
        .expect("merging should succeed");

        let sizes = page_dimensions(&merged).unwrap();
        assert_eq!(sizes.len(), 2);
        for size in &sizes {
            assert!(
                (size.width - 595.0).abs() < 1.0 && (size.height - 842.0).abs() < 1.0,
                "page lost its inherited size: {}x{}",
                size.width,
                size.height
            );
        }
    }

    #[test]
    fn refuses_to_merge_nothing() {
        assert!(merge(&[]).is_err());
    }

    #[test]
    fn reports_which_document_failed_to_load() {
        let error = merge(&[create_blank(&[A4]).unwrap(), b"not a PDF".to_vec()])
            .expect_err("the second document is not a PDF");

        assert!(error.contains("document 2"), "got: {error}");
    }

    #[test]
    fn selects_pages_by_number() {
        let source = create_blank(&[A4, A4, A4, A4]).unwrap();

        let selected = select_pages(&source, &[0, 2]).expect("selecting should succeed");

        assert_eq!(inspect(&selected).unwrap().page_count, 2);
        assert_eq!(page_dimensions(&selected).unwrap().len(), 2);
    }

    #[test]
    fn selecting_keeps_an_inherited_size() {
        let source = document_with_inherited_media_box(3);

        let selected = select_pages(&source, &[1]).unwrap();

        let sizes = page_dimensions(&selected).unwrap();
        assert_eq!(sizes.len(), 1);
        assert!((sizes[0].width - 595.0).abs() < 1.0);
    }

    /// Pins the engine's page convention.
    ///
    /// Every page argument here is ZERO-based, matching render and extract,
    /// even though lopdf's own page map counts from one. Two conventions in
    /// one engine is how an off-by-one gets built in, so index 1 must be the
    /// SECOND page — proved with three differently-sized pages rather than a
    /// count, which would pass either way.
    #[test]
    fn addresses_pages_from_zero() {
        let source = create_blank(&[A4, A5, A4]).unwrap();

        let first = select_pages(&source, &[0]).unwrap();
        let second = select_pages(&source, &[1]).unwrap();

        assert!((page_dimensions(&first).unwrap()[0].width - A4.0).abs() < 1.0);
        assert!(
            (page_dimensions(&second).unwrap()[0].width - A5.0).abs() < 1.0,
            "index 1 should be the second page"
        );
    }

    #[test]
    fn refuses_a_page_that_does_not_exist() {
        let source = create_blank(&[A4, A4]).unwrap();

        let error = select_pages(&source, &[5]).expect_err("page 5 should not exist");
        assert!(error.contains("does not exist"), "got: {error}");
        assert!(error.contains('2'), "should say how many: {error}");
    }

    #[test]
    fn refuses_an_empty_selection() {
        assert!(select_pages(&create_blank(&[A4]).unwrap(), &[]).is_err());
    }

    #[test]
    fn splits_into_one_document_per_page() {
        let source = create_blank(&[A4, A5, A4]).unwrap();

        let parts = split(&source).expect("splitting should succeed");

        assert_eq!(parts.len(), 3);
        for part in &parts {
            assert_eq!(inspect(part).unwrap().page_count, 1);
        }
        // Each part keeps ITS page's size, not the first one's.
        assert!((page_dimensions(&parts[1]).unwrap()[0].width - A5.0).abs() < 1.0);
    }

    #[test]
    fn rotates_every_page() {
        let source = create_blank(&[A4, A4]).unwrap();

        let rotated = rotate(&source, 90, None).expect("rotating should succeed");

        let document = Document::load_mem(&rotated).unwrap();
        for page_id in document.get_pages().into_values() {
            let angle = document
                .get_dictionary(page_id)
                .and_then(|page| page.get(b"Rotate"))
                .and_then(Object::as_i64)
                .unwrap();
            assert_eq!(angle, 90);
        }
    }

    /// Rotation adds to what the page already carries — a document can arrive
    /// with pages already turned, and replacing the value would straighten
    /// them instead of turning them further.
    #[test]
    fn rotation_accumulates_and_wraps() {
        let source = create_blank(&[A4]).unwrap();

        let once = rotate(&source, 270, None).unwrap();
        let twice = rotate(&once, 180, None).unwrap();

        let document = Document::load_mem(&twice).unwrap();
        let page_id = document.get_pages().into_values().next().unwrap();
        let angle = document
            .get_dictionary(page_id)
            .and_then(|page| page.get(b"Rotate"))
            .and_then(Object::as_i64)
            .unwrap();
        // 270 + 180 = 450, normalised into 0..360 rather than left negative
        // or out of range.
        assert_eq!(angle, 90);
    }

    #[test]
    fn rotates_only_the_pages_asked_for() {
        let source = create_blank(&[A4, A4, A4]).unwrap();

        let rotated = rotate(&source, 90, Some(&[1])).unwrap();

        let document = Document::load_mem(&rotated).unwrap();
        let angles: Vec<i64> = document
            .get_pages()
            .into_values()
            .map(|page_id| {
                document
                    .get_dictionary(page_id)
                    .and_then(|page| page.get(b"Rotate"))
                    .and_then(Object::as_i64)
                    .unwrap_or(0)
            })
            .collect();
        assert_eq!(angles, vec![0, 90, 0]);
    }

    #[test]
    fn refuses_an_angle_that_is_not_a_quarter_turn() {
        let source = create_blank(&[A4]).unwrap();

        assert!(rotate(&source, 45, None).is_err());
        assert!(rotate(&source, 1, None).is_err());
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(select_pages(b"not a PDF", &[0]).is_err());
        assert!(split(b"not a PDF").is_err());
        assert!(rotate(b"not a PDF", 90, None).is_err());
    }
}
