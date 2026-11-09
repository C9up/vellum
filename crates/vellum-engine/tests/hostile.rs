//! Documents built to hurt the reader rather than to be read.
//!
//! Every input here arrives from an upload. A panic is caught at the binding,
//! but a stack overflow or an allocation failure is not: it ends the host
//! process. Each case below once did, and each must now come back as an error
//! or as a bounded result.

use vellum_engine::{
    extract_text, fill_form, flatten_form, form_fields, page_dimensions, render_page,
    verify_signatures, FieldValue, RenderOptions, TrustOptions,
};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

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

fn stream(dictionary: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dictionary} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// One A4 page drawing `content`, with `resources` inside its /Resources.
fn one_page(content: &[u8], resources: &str) -> Vec<u8> {
    build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << {resources} >> >>"
        )
        .into_bytes(),
        stream("", content),
    ])
}

const HELVETICA: &str = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>";

/// Deep enough to overflow any thread's stack through an unbounded recursion:
/// 200 000 levels is a 400 KB file.
const DEEP: usize = 200_000;

fn deep_array_in_resources() -> Vec<u8> {
    one_page(
        b"q Q",
        &format!("/Foo {}{}", "[".repeat(DEEP), "]".repeat(DEEP)),
    )
}

fn deep_dictionary_in_resources() -> Vec<u8> {
    one_page(
        b"q Q",
        &format!("/Foo {}1{}", "<< /A ".repeat(DEEP), " >>".repeat(DEEP)),
    )
}

fn deep_array_in_content() -> Vec<u8> {
    let content = format!("{}{} pop q Q", "[".repeat(DEEP), "]".repeat(DEEP));
    one_page(content.as_bytes(), HELVETICA)
}

/// A page whose own dictionary is too deep to read is not a page: it is
/// dropped from the tree, and asking for it is an error.
fn assert_unreadable_page(pdf: &[u8]) {
    let pages = page_dimensions(pdf).expect("the rest of the document is still read");
    assert!(
        pages.is_empty(),
        "the unreadable page was reported: {pages:?}"
    );
    let render = render_page(pdf, 0, &RenderOptions::default());
    assert!(render.is_err(), "rendering an unreadable page succeeded");
    assert!(
        extract_text(pdf, 0).is_err(),
        "reading an unreadable page succeeded"
    );
}

/// A page that is readable but carries a structure too deep for one of its
/// parts still renders, without that part, the way a viewer does.
fn assert_renders(pdf: &[u8]) {
    let png = render_page(pdf, 0, &RenderOptions::default()).expect("the page renders");
    assert!(png.starts_with(PNG_SIGNATURE), "not a PNG");
    extract_text(pdf, 0).expect("the page's text is read");
}

#[test]
fn a_deeply_nested_array_does_not_overflow_the_stack() {
    assert_unreadable_page(&deep_array_in_resources());
}

#[test]
fn a_deeply_nested_dictionary_does_not_overflow_the_stack() {
    assert_unreadable_page(&deep_dictionary_in_resources());
}

#[test]
fn deep_nesting_inside_a_content_stream_does_not_overflow_the_stack() {
    let pdf = deep_array_in_content();
    assert_renders(&pdf);
    assert_eq!(extract_text(&pdf, 0).as_deref(), Ok(""));
}

/// A Separation colour space whose tint transform is a PostScript calculator
/// function nested `DEEP` braces deep.
fn deep_postscript_function() -> Vec<u8> {
    let body = format!("{{{}{} pop 0 }}", "{".repeat(DEEP), "}".repeat(DEEP));
    build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /ColorSpace << /CS0 [/Separation /Ink /DeviceGray 5 0 R] >> >> >>".to_vec(),
        stream("", b"/CS0 cs 0.5 sc 0 0 100 100 re f"),
        stream("/FunctionType 4 /Domain [0 1] /Range [0 1]", body.as_bytes()),
    ])
}

#[test]
fn a_deeply_nested_postscript_function_does_not_overflow_the_stack() {
    assert_renders(&deep_postscript_function());
}

fn page_with_colour_space(colour_space: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /ColorSpace << /CS0 5 0 R >> >> >>".to_vec(),
        stream("", b"/CS0 cs 0.5 sc 0 0 100 100 re f"),
        colour_space.to_vec(),
    ];
    objects.extend_from_slice(extra);
    build_pdf(&objects)
}

/// `count` objects from number 6 on, each naming the next through `link`,
/// the last one being `end`.
fn chain(count: usize, link: impl Fn(usize) -> String, end: &[u8]) -> Vec<Vec<u8>> {
    let mut objects: Vec<Vec<u8>> = (0..count).map(|k| link(7 + k).into_bytes()).collect();
    objects.push(end.to_vec());
    objects
}

const CHAIN: usize = 100_000;

#[test]
fn a_long_chain_of_stitching_functions_does_not_overflow_the_stack() {
    let functions = chain(
        CHAIN,
        |next| {
            format!("<< /FunctionType 3 /Domain [0 1] /Functions [{next} 0 R] /Bounds [] /Encode [0 1] >>")
        },
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
    );
    assert_renders(&page_with_colour_space(
        b"[/Separation /Ink /DeviceGray 6 0 R]",
        &functions,
    ));
}

#[test]
fn a_long_chain_of_indexed_colour_spaces_does_not_overflow_the_stack() {
    let spaces = chain(
        CHAIN,
        |next| format!("[/Indexed {next} 0 R 0 <00>]"),
        b"/DeviceGray",
    );
    assert_renders(&page_with_colour_space(b"[/Indexed 6 0 R 0 <00>]", &spaces));
}

#[test]
fn a_long_chain_of_references_does_not_overflow_the_stack() {
    let references = chain(CHAIN, |next| format!("{next} 0 R"), b"/DeviceGray");
    assert_renders(&page_with_colour_space(b"6 0 R", &references));
}

#[test]
fn a_colour_space_naming_itself_does_not_loop() {
    assert_renders(&page_with_colour_space(b"[/Indexed 5 0 R 0 <00>]", &[]));
}

/// A document whose form is a chain of `depth` fields, each the only child of
/// the one before, ending in a text field. The field tree is the document's to
/// choose; walking it by recursion made its depth the stack's problem.
fn field_chain(depth: usize) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
    ];
    for level in 0..depth {
        objects.push(format!("<< /T (a) /Kids [{} 0 R] >>", 5 + level).into_bytes());
    }
    objects.push(b"<< /T (leaf) /FT /Tx /V (x) >>".to_vec());
    build_pdf(&objects)
}

fn assert_form_refused(pdf: &[u8], reason: &str) {
    let fields = form_fields(pdf).expect_err("reading the form should be refused");
    assert!(fields.contains(reason), "got {fields}");
    let value = FieldValue {
        name: "a.leaf".to_string(),
        value: "y".to_string(),
    };
    let filled = fill_form(pdf, &[value]).expect_err("filling should be refused");
    assert!(filled.contains(reason), "got {filled}");
    let flattened = flatten_form(pdf).expect_err("flattening should be refused");
    assert!(flattened.contains(reason), "got {flattened}");
    let verified =
        verify_signatures(pdf, &TrustOptions::default()).expect_err("verifying should be refused");
    assert!(verified.contains(reason), "got {verified}");
}

#[test]
fn a_field_chain_30_000_deep_is_refused_without_overflowing_the_stack() {
    assert_form_refused(&field_chain(30_000), "deeper than 64 levels");
}

#[test]
fn a_field_chain_10_000_deep_is_refused_before_its_names_grow_quadratically() {
    assert_form_refused(&field_chain(10_000), "deeper than 64 levels");
}

#[test]
fn a_field_tree_of_ordinary_depth_is_still_read() {
    let fields = form_fields(&field_chain(10)).expect("a ten-level form is read");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, format!("{}leaf", "a.".repeat(10)));
    assert_eq!(fields[0].value.as_deref(), Some("x"));
}

#[test]
fn fields_inheriting_one_huge_option_list_are_refused() {
    // One megabyte of options, inherited by a hundred fields, would be a
    // hundred megabytes of strings handed back for a one-megabyte file.
    let options: Vec<String> = (0..1000).map(|i| format!("({i:0>1000})")).collect();
    let kids: Vec<String> = (0..100).map(|i| format!("{} 0 R", 6 + i)).collect();
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
        format!("[{}]", options.join(" ")).into_bytes(),
        format!(
            "<< /T (list) /FT /Ch /Opt 4 0 R /Kids [{}] >>",
            kids.join(" ")
        )
        .into_bytes(),
    ];
    for i in 0..100 {
        objects.push(format!("<< /T (f{i}) /Parent 5 0 R >>").into_bytes());
    }
    let refused = form_fields(&build_pdf(&objects)).expect_err("the form should be refused");
    assert!(refused.contains("MiB of text"), "got {refused}");
}

#[test]
fn a_parent_chain_without_end_does_not_loop() {
    // Two fields naming each other as /Parent, neither declaring a type.
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
        b"<< /T (a) /Parent 5 0 R >>".to_vec(),
        b"<< /T (b) /Parent 4 0 R >>".to_vec(),
    ]);
    assert_eq!(form_fields(&pdf), Ok(Vec::new()));
}
