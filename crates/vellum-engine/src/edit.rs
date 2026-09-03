//! Operations on documents that already exist: merge, select, split, rotate.
//!
//! All of these move pages between page trees, which is where PDF hides a
//! trap: `Resources`, `MediaBox`, `CropBox` and `Rotate` may live on a parent
//! `Pages` node and be INHERITED by the page (PDF 32000-1 §7.7.3.4). Re-parent
//! such a page and it silently loses its size and its fonts. Every operation
//! here materialises the inherited attributes onto the page first.

use std::collections::{BTreeMap, HashSet};

use lopdf::{dictionary, Document, Object, ObjectId};

/// Attributes a page inherits from its ancestors in the page tree.
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn load(bytes: &[u8]) -> Result<Document, String> {
    Document::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))
}

fn save(mut document: Document) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out)
}

/// Walk up from each page and copy any inherited attribute onto the page.
///
/// Without this, a page whose `MediaBox` lives on its parent comes out of a
/// merge or a split with no size at all — which readers fall back to Letter
/// for, quietly resizing a French A4 document.
fn flatten_inheritance(document: &mut Document) {
    let page_ids: Vec<ObjectId> = document.get_pages().into_values().collect();

    for page_id in page_ids {
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
            // document can carry and which would otherwise spin forever.
            let mut seen = HashSet::new();
            let mut next = page.get(b"Parent").and_then(Object::as_reference).ok();
            while let Some(parent_id) = next {
                if !seen.insert(parent_id) {
                    break;
                }
                let Ok(parent) = document.get_dictionary(parent_id) else {
                    break;
                };
                missing.retain(|key| match parent.get(key) {
                    Ok(value) => {
                        inherited.push((key.to_vec(), value.clone()));
                        false
                    }
                    Err(_) => true,
                });
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
}

/// Drop references to objects that no longer exist from every `Kids` array.
///
/// `Document::delete_pages` removes the page object and decrements `Count`,
/// but leaves the dangling reference behind. A reader resolves it to null and
/// may show a phantom page, so the arrays are tidied here.
fn prune_dangling_kids(document: &mut Document) {
    let page_tree_ids: Vec<ObjectId> = document
        .objects
        .iter()
        .filter(|(_, object)| {
            object
                .as_dict()
                .and_then(|dict| dict.get(b"Type"))
                .and_then(Object::as_name)
                .is_ok_and(|name| name == b"Pages")
        })
        .map(|(id, _)| *id)
        .collect();

    for tree_id in page_tree_ids {
        let Ok(kids) = document
            .get_dictionary(tree_id)
            .and_then(|dict| dict.get(b"Kids"))
            .and_then(Object::as_array)
        else {
            continue;
        };

        let kept: Vec<Object> = kids
            .iter()
            .filter(|kid| match kid.as_reference() {
                Ok(id) => document.objects.contains_key(&id),
                // A kid written inline rather than as a reference cannot dangle.
                Err(_) => true,
            })
            .cloned()
            .collect();

        if kept.len() != kids.len() {
            if let Ok(dict) = document
                .get_object_mut(tree_id)
                .and_then(|object| object.as_dict_mut())
            {
                dict.set("Kids", kept);
            }
        }
    }
}

/// Join documents end to end, in the order given.
pub fn merge(documents: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if documents.is_empty() {
        return Err("merging needs at least one document".to_string());
    }

    let mut max_id = 1;
    let mut pages = BTreeMap::new();
    let mut objects = BTreeMap::new();

    for (index, bytes) in documents.iter().enumerate() {
        let mut document =
            load(bytes).map_err(|error| format!("document {}: {error}", index + 1))?;
        flatten_inheritance(&mut document);

        // Every document numbers its objects from 1, so they are shifted past
        // the ones already collected before anything is merged.
        document.renumber_objects_with(max_id);
        max_id = document.max_id + 1;

        for page_id in document.get_pages().into_values() {
            if let Ok(page) = document.get_object(page_id) {
                pages.insert(page_id, page.to_owned());
            }
        }
        objects.extend(document.objects);
    }

    if pages.is_empty() {
        return Err("none of the documents has a page".to_string());
    }

    let mut merged = Document::with_version("1.7");
    let mut catalog_id = None;
    let mut pages_id = None;

    for (object_id, object) in objects {
        let object_type = object
            .as_dict()
            .and_then(|dict| dict.get(b"Type"))
            .and_then(Object::as_name)
            .unwrap_or(b"");

        match object_type {
            b"Catalog" => catalog_id = catalog_id.or(Some(object_id)),
            b"Pages" => pages_id = pages_id.or(Some(object_id)),
            // Pages are re-parented below; outlines refer to the page trees we
            // are dismantling, so they are dropped rather than left dangling.
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                merged.objects.insert(object_id, object);
            }
        }
    }

    let pages_id = pages_id.ok_or_else(|| "no page tree found".to_string())?;
    let catalog_id = catalog_id.ok_or_else(|| "no catalog found".to_string())?;

    for (page_id, page) in &pages {
        if let Ok(dictionary) = page.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_id);
            merged
                .objects
                .insert(*page_id, Object::Dictionary(dictionary));
        }
    }

    let kids: Vec<Object> = pages.keys().copied().map(Object::Reference).collect();
    merged.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => kids.len() as i64,
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

    merged.max_id = max_id;
    merged.renumber_objects();
    merged.prune_objects();

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
    flatten_inheritance(&mut document);

    let available: Vec<u32> = document.get_pages().into_keys().collect();
    let total = available.len();
    if let Some(missing) = pages.iter().find(|page| !available.contains(&(*page + 1))) {
        return Err(format!(
            "page {} does not exist — the document has {total}",
            missing + 1
        ));
    }

    let wanted: HashSet<u32> = pages.iter().map(|page| page + 1).collect();
    let to_delete: Vec<u32> = available
        .into_iter()
        .filter(|page| !wanted.contains(page))
        .collect();

    document.delete_pages(&to_delete);
    prune_dangling_kids(&mut document);
    document.prune_objects();

    save(document)
}

/// One single-page document per page, in document order.
pub fn split(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut document = load(bytes)?;
    flatten_inheritance(&mut document);
    let page_numbers: Vec<u32> = document.get_pages().into_keys().collect();

    page_numbers
        .iter()
        .map(|page| {
            // Cloned rather than re-parsed for each page: the parse is the
            // expensive half, and a 200-page document would pay it 200 times.
            let mut single = document.clone();
            let others: Vec<u32> = page_numbers
                .iter()
                .copied()
                .filter(|other| other != page)
                .collect();
            single.delete_pages(&others);
            prune_dangling_kids(&mut single);
            single.prune_objects();
            save(single)
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
    let all = document.get_pages();
    let total = all.len();

    let targets: Vec<ObjectId> = match pages {
        Some(wanted) => {
            if let Some(missing) = wanted.iter().find(|page| !all.contains_key(&(*page + 1))) {
                return Err(format!(
                    "page {} does not exist — the document has {total}",
                    missing + 1
                ));
            }
            wanted
                .iter()
                .filter_map(|page| all.get(&(page + 1)).copied())
                .collect()
        }
        None => all.into_values().collect(),
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
