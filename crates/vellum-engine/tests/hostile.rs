//! Documents built to hurt the reader rather than to be read.
//!
//! Every input here arrives from an upload. A panic is caught at the binding,
//! but a stack overflow or an allocation failure is not: it ends the host
//! process. Each case below once did, and each must now come back as an error
//! or as a bounded result.

use std::io::Write;
use std::time::{Duration, Instant};

use vellum_engine::{
    extract_text, extract_text_all, extract_text_all_with, extract_text_with, fill_form,
    flatten_form, form_fields, page_dimensions, render_all, render_page, verify_signatures,
    FieldValue, ReadLimits, RenderOptions, TrustOptions,
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

const OBJECTS: &str = "arrays or dictionaries more than 512 levels deep";
const CHAINS: &str = "colour spaces or functions more than 64 levels deep";
const PROCEDURES: &str = "calculator procedures more than 64 levels deep";

/// Where a document nests too deep.
#[derive(Clone, Copy)]
enum Nesting {
    /// In the objects that make up the page tree.
    InStructure,
    /// In a content stream, which only drawing and reading text interpret.
    InContent,
}

/// Nesting past the reader's bound is refused by every way of reading the
/// part that holds it, never answered as a page that silently lacks it.
fn assert_refused_as_too_deep(pdf: &[u8], nesting: Nesting, what: &str) {
    let dimensions = page_dimensions(pdf);
    match nesting {
        Nesting::InStructure => {}
        // Measuring a page reads no content, so it has nothing to refuse.
        Nesting::InContent => assert_eq!(
            dimensions.as_ref().map(Vec::len),
            Ok(1),
            "the page was not measured"
        ),
    }
    let mut refusals = vec![
        (
            "render_page",
            render_page(pdf, 0, &RenderOptions::default()).map(|_| ()),
        ),
        (
            "render_all",
            render_all(pdf, &RenderOptions::default()).map(|_| ()),
        ),
        ("extract_text", extract_text(pdf, 0).map(|_| ())),
        ("extract_text_all", extract_text_all(pdf).map(|_| ())),
    ];
    if let Nesting::InStructure = nesting {
        refusals.push(("page_dimensions", dimensions.map(|_| ())));
    }
    for (call, result) in refusals {
        let message = result.expect_err(&format!("{call} accepted the nesting"));
        assert_eq!(message, format!("the document nests {what}"), "{call}");
    }
}

/// A page that is readable but carries a structure too deep for one of its
/// parts still renders, without that part, the way a viewer does.
fn assert_renders(pdf: &[u8]) {
    let png = render_page(pdf, 0, &RenderOptions::default()).expect("the page renders");
    assert!(png.starts_with(PNG_SIGNATURE), "not a PNG");
    extract_text(pdf, 0).expect("the page's text is read");
}

#[test]
fn a_deeply_nested_array_is_refused_without_overflowing_the_stack() {
    assert_refused_as_too_deep(&deep_array_in_resources(), Nesting::InStructure, OBJECTS);
}

#[test]
fn a_deeply_nested_dictionary_is_refused_without_overflowing_the_stack() {
    assert_refused_as_too_deep(
        &deep_dictionary_in_resources(),
        Nesting::InStructure,
        OBJECTS,
    );
}

#[test]
fn deep_nesting_inside_a_content_stream_is_refused_without_overflowing_the_stack() {
    assert_refused_as_too_deep(&deep_array_in_content(), Nesting::InContent, OBJECTS);
}

#[test]
fn nesting_just_past_the_bound_is_refused_and_just_under_it_is_read() {
    let nested = |depth: usize| {
        let content = format!("{}{} pop q Q", "[".repeat(depth), "]".repeat(depth));
        one_page(content.as_bytes(), HELVETICA)
    };
    assert_refused_as_too_deep(&nested(513), Nesting::InContent, OBJECTS);
    assert_renders(&nested(500));
    let resources = |depth: usize| {
        one_page(
            b"q Q",
            &format!("/Foo {}1{}", "<< /A ".repeat(depth), " >>".repeat(depth)),
        )
    };
    assert_refused_as_too_deep(&resources(520), Nesting::InStructure, OBJECTS);
    assert_renders(&resources(400));
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
fn a_deeply_nested_postscript_function_is_refused_without_overflowing_the_stack() {
    assert_refused_as_too_deep(&deep_postscript_function(), Nesting::InContent, PROCEDURES);
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
fn a_long_chain_of_stitching_functions_is_refused_without_overflowing_the_stack() {
    let functions = chain(
        CHAIN,
        |next| {
            format!("<< /FunctionType 3 /Domain [0 1] /Functions [{next} 0 R] /Bounds [] /Encode [0 1] >>")
        },
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
    );
    assert_refused_as_too_deep(
        &page_with_colour_space(b"[/Separation /Ink /DeviceGray 6 0 R]", &functions),
        Nesting::InContent,
        CHAINS,
    );
}

#[test]
fn a_long_chain_of_indexed_colour_spaces_is_refused_without_overflowing_the_stack() {
    let spaces = chain(
        CHAIN,
        |next| format!("[/Indexed {next} 0 R 0 <00>]"),
        b"/DeviceGray",
    );
    assert_refused_as_too_deep(
        &page_with_colour_space(b"[/Indexed 6 0 R 0 <00>]", &spaces),
        Nesting::InContent,
        CHAINS,
    );
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

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(data).expect("compressing in memory");
    encoder.finish().expect("compressing in memory")
}

const MIB: usize = 1024 * 1024;

/// A budget small enough that the tests never allocate much themselves.
fn one_mib() -> ReadLimits {
    ReadLimits {
        max_decoded_bytes: MIB as u64,
        ..ReadLimits::default()
    }
}

fn render_with(limits: ReadLimits) -> RenderOptions {
    RenderOptions {
        limits,
        ..RenderOptions::default()
    }
}

/// A page whose content is `copies` references to one Flate stream of
/// `inflated` bytes of whitespace.
fn flate_bomb(inflated: usize, copies: usize) -> Vec<u8> {
    let compressed = deflate(&vec![b' '; inflated]);
    let contents = vec!["4 0 R"; copies].join(" ");
    build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents [{contents}] /Resources << {HELVETICA} >> >>"
        )
        .into_bytes(),
        stream("/Filter /FlateDecode", &compressed),
    ])
}

/// A document whose object 5 sits in object stream 4, which inflates to
/// `inflated` bytes, found through a cross-reference stream when `indexed`;
/// otherwise the object stream is there but nothing points into it.
fn object_stream_bomb(inflated: usize, indexed: bool) -> Vec<u8> {
    let mut content = b"5 0 << /Kept true >>".to_vec();
    content.resize(inflated, b' ');
    let bodies: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
        stream(
            "/Type /ObjStm /N 1 /First 4 /Filter /FlateDecode",
            &deflate(&content),
        ),
    ];
    if !indexed {
        return build_pdf(&bodies);
    }

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    // Type, field 2 (4 bytes), field 3 (2 bytes), for objects 0 to 6.
    let mut entries = vec![0u8, 0, 0, 0, 0, 0xff, 0xff];
    for offset in &offsets {
        entries.push(1);
        entries.extend_from_slice(&u32::try_from(*offset).unwrap().to_be_bytes());
        entries.extend_from_slice(&[0, 0]);
    }
    entries.extend_from_slice(&[2, 0, 0, 0, 4, 0, 0]);
    entries.push(1);
    entries.extend_from_slice(&u32::try_from(xref_at).unwrap().to_be_bytes());
    entries.extend_from_slice(&[0, 0]);
    out.extend_from_slice(b"6 0 obj\n");
    out.extend_from_slice(&stream(
        "/Type /XRef /Size 7 /W [1 4 2] /Root 1 0 R",
        &entries,
    ));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

/// lopdf inflates every object stream while it parses, before anything
/// else runs, on every path that parses: 1 MiB of upload inflating to a
/// gigabyte took the process down. Past the ceiling the stream is not
/// inflated, and a document missing what the stream held is refused.
#[test]
fn an_object_stream_inflating_past_the_ceiling_is_refused() {
    let pdf = object_stream_bomb(64 * MIB, true);
    let started = Instant::now();
    let error = vellum_engine::inspect(&pdf).expect_err("refused");
    assert!(error.contains("object stream 4"), "got {error}");
    assert!(form_fields(&pdf).is_err());
    assert!(vellum_engine::is_signed(&pdf).is_err());
    assert!(started.elapsed() < Duration::from_secs(30));

    // Under the ceiling, the same document reads.
    let pdf = object_stream_bomb(MIB, true);
    assert_eq!(vellum_engine::inspect(&pdf).expect("read").page_count, 1);
}

/// One nothing points into is not inflated past the ceiling either, and its
/// absence takes nothing from the document.
#[test]
fn an_object_stream_nothing_points_into_is_not_inflated_past_the_ceiling() {
    let pdf = object_stream_bomb(64 * MIB, false);
    assert_eq!(vellum_engine::inspect(&pdf).expect("read").page_count, 1);
}

#[test]
fn a_flate_stream_inflating_past_the_budget_is_refused() {
    let pdf = flate_bomb(16 * MIB, 1);
    let rendered = render_page(&pdf, 0, &render_with(one_mib())).expect_err("refused");
    assert!(rendered.contains("more than 1 MiB"), "got {rendered}");
    let text = extract_text_with(&pdf, 0, &one_mib()).expect_err("refused");
    assert!(text.contains("raise maxDecodedBytes"), "got {text}");
}

#[test]
fn one_stream_referenced_many_times_is_charged_every_time() {
    // 256 KiB fits the budget once; eight references to it do not.
    let pdf = flate_bomb(256 * 1024, 8);
    assert!(render_page(&pdf, 0, &render_with(one_mib())).is_err());
    let single = flate_bomb(256 * 1024, 1);
    assert!(render_page(&single, 0, &render_with(one_mib())).is_ok());
}

#[test]
fn a_run_length_stream_inflating_past_the_budget_is_refused() {
    // Each pair of bytes repeats a space 128 times: 32 KiB become 2 MiB.
    let encoded: Vec<u8> = [129u8, b' '].repeat(16 * 1024);
    let pdf = one_page_with_filter("/Filter /RunLengthDecode", &encoded);
    assert!(render_page(&pdf, 0, &render_with(one_mib())).is_err());
}

fn one_page_with_filter(dictionary: &str, data: &[u8]) -> Vec<u8> {
    build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << {HELVETICA} >> >>"
        )
        .into_bytes(),
        stream(dictionary, data),
    ])
}

#[test]
fn an_image_declaring_more_pixels_than_the_budget_is_refused() {
    // 60 000 x 60 000 RGB declared, a few kilobytes on the wire.
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 60000 /Height 60000 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
        &deflate(&vec![0u8; 4 * MIB]),
    );
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >>".to_vec(),
        stream("", b"q 595 0 0 842 0 0 cm /Im Do Q"),
        image,
    ]);
    assert!(render_page(&pdf, 0, &render_with(one_mib())).is_err());
}

#[test]
fn an_ordinary_page_reads_within_the_default_budget() {
    let pdf = flate_bomb(64 * 1024, 2);
    assert!(render_page(&pdf, 0, &RenderOptions::default()).is_ok());
}

/// Form XObjects each drawing the next ten times, `levels` deep: 10^levels
/// leaf drawings from a file of two kilobytes.
fn xobject_fan_out(levels: usize) -> Vec<u8> {
    let mut forms = Vec::new();
    for level in 0..levels {
        let last = level + 1 == levels;
        let body = if last {
            "BT /F1 1 Tf (a) Tj ET 0 0 1 1 re f".to_string()
        } else {
            ["/X Do"; 10].join(" ")
        };
        let resources = if last {
            HELVETICA.to_string()
        } else {
            format!("/XObject << /X {} 0 R >>", 6 + level)
        };
        forms.push(stream(
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << {resources} >>"
            ),
            body.as_bytes(),
        ));
    }
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /XObject << /X 5 0 R >> >> >>".to_vec(),
        stream("", b"/X Do"),
    ];
    objects.extend(forms);
    build_pdf(&objects)
}

#[test]
fn an_xobject_fan_out_is_refused_once_the_operations_run_out() {
    // A billion leaf drawings; the budget stops the interpretation after a
    // million operations, instead of minutes later.
    let pdf = xobject_fan_out(9);
    let limits = ReadLimits {
        max_operations: 1_000_000,
        ..ReadLimits::default()
    };
    let started = Instant::now();
    let rendered = render_page(&pdf, 0, &render_with(limits)).expect_err("refused");
    assert!(rendered.contains("raise maxOperations"), "got {rendered}");
    let text = extract_text_with(&pdf, 0, &limits).expect_err("refused");
    assert!(text.contains("more than 1000000 operations"), "got {text}");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn the_operation_budget_is_the_callers_to_set() {
    let pdf = xobject_fan_out(3);
    let tight = ReadLimits {
        max_operations: 100,
        ..ReadLimits::default()
    };
    assert!(render_page(&pdf, 0, &render_with(tight)).is_err());
    assert!(render_page(&pdf, 0, &RenderOptions::default()).is_ok());
}

#[test]
fn each_page_of_a_long_document_has_a_budget_of_its_own() {
    // Three pages of 512 KiB each: 1.5 MiB in all, under 1 MiB per page.
    let compressed = deflate(&vec![b' '; 512 * 1024]);
    let pdf = build_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 6 0 R >>".to_vec(),
        stream("/Filter /FlateDecode", &compressed),
    ]);
    assert_eq!(
        render_all(&pdf, &render_with(one_mib())).map(|pages| pages.len()),
        Ok(3)
    );
    assert_eq!(
        extract_text_all_with(&pdf, &one_mib()).map(|pages| pages.len()),
        Ok(3)
    );
}

#[test]
fn render_all_refuses_a_document_whose_pages_add_up_past_the_ceiling() {
    // Forty pages of 7000 x 7000 points: each just under the per-page
    // ceiling, two billion pixels between them.
    let kids: Vec<String> = (0..40).map(|page| format!("{} 0 R", 4 + page)).collect();
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count 40 >>", kids.join(" ")).into_bytes(),
        stream("", b"0 0 7000 7000 re f"),
    ];
    for _ in 0..40 {
        objects.push(
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 7000 7000] /Contents 3 0 R >>".to_vec(),
        );
    }
    let started = Instant::now();
    let refused = render_all(&build_pdf(&objects), &RenderOptions::default()).expect_err("refused");
    assert!(refused.contains("raise maxTotalPixels"), "got {refused}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it was refused only after drawing: {:?}",
        started.elapsed()
    );
}

/// A form of `fields` signature fields, whose values are built by `value`,
/// over a document padded to more than 4 MB so that hashing it costs.
fn signature_fields(fields: usize, value: impl Fn(usize) -> String) -> Vec<u8> {
    let names: Vec<String> = (0..fields)
        .map(|field| format!("{} 0 R", 6 + field))
        .collect();
    let mut objects = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] /SigFlags 3 >> >>",
            names.join(" ")
        )
        .into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
        stream("", &vec![b' '; 4 * MIB + 1024]),
        b"<< /Type /Sig /ByteRange [0 10 20 4000000] /Contents <00> >>".to_vec(),
    ];
    for field in 0..fields {
        objects.push(format!("<< /FT /Sig /T (s{field}) /V {} >>", value(field)).into_bytes());
    }
    build_pdf(&objects)
}

#[test]
fn thousands_of_fields_sharing_one_signature_hash_it_once() {
    let pdf = signature_fields(5000, |_| "5 0 R".to_string());
    let started = Instant::now();
    let reports = verify_signatures(&pdf, &TrustOptions::default()).expect("verified");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(reports.len(), 5000);
    assert_eq!(reports[4999].field, "s4999");
    assert_eq!(reports[0].problems, reports[4999].problems);
}

#[test]
fn signatures_past_the_hundredth_are_reported_unchecked() {
    let pdf = signature_fields(150, |_| {
        "<< /Type /Sig /ByteRange [0 10 20 4000000] /Contents <00> >>".to_string()
    });
    let reports = verify_signatures(&pdf, &TrustOptions::default()).expect("verified");
    assert_eq!(reports.len(), 150);
    assert!(!reports[99]
        .problems
        .iter()
        .any(|p| p.starts_with("not checked")));
    assert!(
        reports[100].problems[0].starts_with("not checked"),
        "{:?}",
        reports[100].problems
    );
}
