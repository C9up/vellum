//! Editing a document costs what the document weighs, not its page count
//! raised to a power.
//!
//! Splitting, selecting and stamping were built on lopdf primitives that each
//! walk or clone the whole document once per page: a 75 KB file of a thousand
//! pages held a worker for a minute in `split`, and one page listed twenty
//! thousand times in the page tree held it for two in `stamp_text`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use vellum_engine::{
    flatten_form, inspect, merge, rotate, select_pages, split, stamp_text, TextStampOptions,
};

/// Assemble a PDF from object bodies numbered from 1, with a correct xref.
fn build_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// `count` minimal pages inheriting their size from the page tree, the
/// shape that made `split` cubic.
fn many_pages(count: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..count).map(|page| format!("{} 0 R", page + 3)).collect();
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {count} /MediaBox [0 0 595 842] >>",
            kids.join(" ")
        )
        .into_bytes(),
    ];
    for _ in 0..count {
        objects.push(b"<< /Type /Page /Parent 2 0 R >>".to_vec());
    }
    build_pdf(&objects)
}

/// One page object listed `times` in /Kids, with enough filler objects that
/// lopdf's own iteration limit lets every entry through.
fn one_page_listed(times: usize) -> Vec<u8> {
    let kids = vec!["3 0 R"; times].join(" ");
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {times} >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
    ];
    objects.extend((0..times).map(|_| b"null".to_vec()));
    build_pdf(&objects)
}

fn assert_quick(what: &str, started: Instant, ceiling: Duration) {
    assert!(
        started.elapsed() < ceiling,
        "{what} took {:?}",
        started.elapsed()
    );
}

#[test]
fn splitting_a_thousand_pages_costs_linear_time() {
    let pdf = many_pages(1000);
    let started = Instant::now();
    let parts = split(&pdf).expect("split");
    assert_quick("split of 1000 pages", started, Duration::from_secs(5));
    assert_eq!(parts.len(), 1000);
    for part in [&parts[0], &parts[499], &parts[999]] {
        assert_eq!(inspect(part).expect("a part reads").page_count, 1);
        // The inherited size came along with the page.
        let sizes = vellum_engine::page_dimensions(part).expect("a part measures");
        assert!((sizes[0].width - 595.0).abs() < 1.0);
    }
}

#[test]
fn selecting_one_page_of_twenty_thousand_costs_linear_time() {
    let pdf = many_pages(20_000);
    let started = Instant::now();
    let selected = select_pages(&pdf, &[19_999, 0]).expect("select");
    assert_quick(
        "selectPages of 20 000 pages",
        started,
        Duration::from_secs(5),
    );
    assert_eq!(inspect(&selected).expect("reads").page_count, 2);
    // Kept in document order, and nothing of the other pages came along.
    assert!(selected.len() < 2048, "{} bytes", selected.len());
}

#[test]
fn merging_twenty_thousand_pages_costs_linear_time() {
    let pdf = many_pages(10_000);
    let started = Instant::now();
    let merged = merge(&[pdf.clone(), pdf]).expect("merge");
    assert_quick("merge of 20 000 pages", started, Duration::from_secs(5));
    assert_eq!(inspect(&merged).expect("reads").page_count, 20_000);
}

#[test]
fn a_page_listed_twice_in_the_page_tree_is_refused() {
    let pdf = one_page_listed(3);
    let refusals = [
        ("inspect", inspect(&pdf).map(|_| ())),
        (
            "stamp_text",
            stamp_text(&pdf, "X", &TextStampOptions::default()).map(|_| ()),
        ),
        ("rotate", rotate(&pdf, 90, None).map(|_| ())),
        ("select_pages", select_pages(&pdf, &[0]).map(|_| ())),
        ("split", split(&pdf).map(|_| ())),
        ("merge", merge(std::slice::from_ref(&pdf)).map(|_| ())),
    ];
    for (what, result) in refusals {
        let error = result.expect_err(what);
        assert!(error.contains("more than once"), "{what}: {error}");
    }
}

#[test]
fn a_page_listed_twenty_thousand_times_is_refused_at_once() {
    let pdf = one_page_listed(20_000);
    let started = Instant::now();
    assert!(stamp_text(&pdf, "X", &TextStampOptions::default()).is_err());
    assert_quick("refusing 20 000 listings", started, Duration::from_secs(5));
}

#[test]
fn a_page_index_past_u32_is_reported_without_wrapping() {
    let pdf = many_pages(3);
    let error = select_pages(&pdf, &[u32::MAX]).expect_err("no such page");
    assert!(error.contains("page 4294967296 does not exist"), "{error}");
    let error = rotate(&pdf, 90, Some(&[u32::MAX])).expect_err("no such page");
    assert!(error.contains("page 4294967296 does not exist"), "{error}");
}

/// The refusals below each build a few hundred megabytes before the
/// allowance runs out; one at a time, so the binary stays within a small
/// machine's memory.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `pages` pages that all share what `shared` describes: `catalog` is added
/// to the catalog, `tree` to the page tree node, `page` to every page, and
/// `extra` is appended as objects 3 + pages onwards.
fn sharing(pages: usize, catalog: &str, tree: &str, page: &str, extra: Vec<Vec<u8>>) -> Vec<u8> {
    let kids: Vec<String> = (0..pages)
        .map(|index| format!("{} 0 R", index + 3))
        .collect();
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 595 842] {tree} >>",
            kids.join(" ")
        )
        .into_bytes(),
    ];
    for _ in 0..pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R {page} >>").into_bytes());
    }
    objects.extend(extra);
    build_pdf(&objects)
}

/// A dictionary of `entries` names, each holding `null`.
fn wide_dictionary(entries: usize) -> String {
    let names: Vec<String> = (0..entries)
        .map(|entry| format!("/F{entry} null"))
        .collect();
    format!("<< {} >>", names.join(" "))
}

/// A thousand pages, a million values the catalog reaches: every part of a
/// split carries them, a billion values from a 2 MB file.
#[test]
fn split_refuses_to_copy_a_shared_catalog_once_per_page() {
    let _serial = one_at_a_time();
    let pages = 1000;
    let big = format!("[{}]", "0 ".repeat(1_000_000)).into_bytes();
    let pdf = sharing(pages, &format!("/Big {} 0 R", pages + 3), "", "", vec![big]);
    let started = Instant::now();
    let error = split(&pdf).expect_err("refused");
    assert_quick("refusing the split", started, Duration::from_secs(10));
    assert!(error.contains("select the pages you need"), "{error}");
    // One page of it is still one copy.
    assert_eq!(
        inspect(&select_pages(&pdf, &[0]).expect("select"))
            .expect("reads")
            .page_count,
        1
    );
}

/// A thousand pages sharing one font group of two hundred thousand entries:
/// stamping wrote the group inline into every page.
#[test]
fn stamping_refuses_to_copy_shared_resources_once_per_page() {
    let _serial = one_at_a_time();
    let pages = 1000;
    let resources = pages + 3;
    let pdf = sharing(
        pages,
        "",
        "",
        &format!("/Resources {resources} 0 R"),
        vec![
            format!("<< /Font {} 0 R >>", resources + 1).into_bytes(),
            wide_dictionary(200_000).into_bytes(),
        ],
    );
    let started = Instant::now();
    let error = stamp_text(&pdf, "X", &TextStampOptions::default()).expect_err("refused");
    assert_quick("refusing the stamp", started, Duration::from_secs(10));
    assert!(error.contains("would copy more than"), "{error}");
    // Stamping one page copies the group once.
    let one = TextStampOptions {
        page: Some(0),
        ..TextStampOptions::default()
    };
    assert!(stamp_text(&pdf, "X", &one).is_ok());
}

/// A thousand pages sharing one /Annots array of two hundred thousand
/// entries: flattening copied it once per page.
#[test]
fn flattening_refuses_to_copy_a_shared_annotation_list_once_per_page() {
    let _serial = one_at_a_time();
    let pages = 1000;
    let entries = vec!["null"; 200_000].join(" ");
    let pdf = sharing(
        pages,
        "",
        "",
        &format!("/Annots {} 0 R", pages + 3),
        vec![format!("[{entries}]").into_bytes()],
    );
    let started = Instant::now();
    let error = flatten_form(&pdf).expect_err("refused");
    assert_quick("refusing to flatten", started, Duration::from_secs(10));
    assert!(error.contains("would copy more than"), "{error}");
}

/// Resources inherited inline from the page tree are copied onto every
/// page a merge re-parents.
#[test]
fn merging_refuses_to_copy_inherited_resources_once_per_page() {
    let _serial = one_at_a_time();
    let pdf = sharing(
        1000,
        "",
        &format!("/Resources << /Font {} >>", wide_dictionary(200_000)),
        "",
        Vec::new(),
    );
    let started = Instant::now();
    let error = merge(std::slice::from_ref(&pdf)).expect_err("refused");
    assert_quick("refusing the merge", started, Duration::from_secs(10));
    assert!(error.contains("would copy more than"), "{error}");
    // Selecting a page carries its inheritance alone.
    assert_eq!(
        inspect(&select_pages(&pdf, &[999]).expect("select"))
            .expect("reads")
            .page_count,
        1
    );
}
