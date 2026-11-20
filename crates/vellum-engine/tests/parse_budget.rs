//! What parsing one document may build is bounded for the document, not per
//! stream.
//!
//! lopdf's own limit bounds what ONE object stream inflates to. A 32 KB file
//! whose single object stream inflated just under it to one array of sixteen
//! million zeros cost 2 GB in `inspect`, and four of them killed a process
//! holding 3 GB. This file is its own test binary so that the peak memory of
//! the process is the peak of these documents alone.

use std::io::Write;
use std::time::{Duration, Instant};

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(data).expect("compressing in memory");
    encoder.finish().expect("compressing in memory")
}

fn stream(dictionary: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dictionary} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A one-page document whose objects `4 + n + i` sit, one each, in object
/// streams `4 + i` holding `bodies[i]`, found through a cross-reference
/// stream, so that every stream is one the document needs.
fn object_streams(bodies: &[Vec<u8>]) -> Vec<u8> {
    let count = bodies.len();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_vec(),
    ];
    for (index, body) in bodies.iter().enumerate() {
        let header = format!("{} 0 ", 4 + count + index);
        let mut content = header.clone().into_bytes();
        content.extend_from_slice(body);
        objects.push(stream(
            &format!(
                "/Type /ObjStm /N 1 /First {} /Filter /FlateDecode",
                header.len()
            ),
            &deflate(&content),
        ));
    }

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_number = 4 + 2 * count;
    let xref_at = out.len();
    // Type, field 2 (4 bytes), field 3 (2 bytes).
    let mut entries = vec![0u8, 0, 0, 0, 0, 0xff, 0xff];
    for offset in &offsets {
        entries.push(1);
        entries.extend_from_slice(&u32::try_from(*offset).expect("small").to_be_bytes());
        entries.extend_from_slice(&[0, 0]);
    }
    for index in 0..count {
        entries.push(2);
        entries.extend_from_slice(&u32::try_from(4 + index).expect("small").to_be_bytes());
        entries.extend_from_slice(&[0, 0]);
    }
    entries.push(1);
    entries.extend_from_slice(&u32::try_from(xref_at).expect("small").to_be_bytes());
    entries.extend_from_slice(&[0, 0]);
    out.extend_from_slice(format!("{xref_number} 0 obj\n").as_bytes());
    out.extend_from_slice(&stream(
        &format!(
            "/Type /XRef /Size {} /W [1 4 2] /Root 1 0 R",
            xref_number + 1
        ),
        &entries,
    ));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

/// A cross-reference stream of one-byte entries: `entries` normal objects
/// declared by as many bytes, inflated from a few kilobytes.
fn one_byte_xref_entries(entries: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let xref_at = out.len();
    let data = deflate(&vec![1u8; entries]);
    out.extend_from_slice(b"1 0 obj\n");
    out.extend_from_slice(&stream(
        &format!("/Type /XRef /Size {entries} /W [1 0 0] /Filter /FlateDecode"),
        &data,
    ));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

/// The most memory this process has held, from the kernel's own count.
#[cfg(target_os = "linux")]
fn peak_resident_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("procfs");
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .expect("VmHWM");
    let kib: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .expect("a number of kB");
    kib * 1024
}

const MIB: usize = 1024 * 1024;

#[cfg(target_os = "linux")]
#[test]
fn what_one_document_may_build_is_bounded_in_total() {
    let started = Instant::now();

    // One stream just under the per-stream cap, holding one array of 16.5
    // million zeros: 32 KB on the wire.
    let mut array = b"[".to_vec();
    array.extend(b"0 ".repeat(16_500_000));
    array.push(b']');
    let pdf = object_streams(&[array]);
    assert!(pdf.len() < 64 * 1024, "{} bytes", pdf.len());
    let error = vellum_engine::inspect(&pdf).expect_err("refused");
    assert!(error.contains("more than 4000000 objects"), "got {error}");
    assert!(vellum_engine::form_fields(&pdf).is_err());

    // Four streams of padding, each just under the per-stream cap: under it
    // one by one, 64 MiB together.
    let mut padding = b"<< /Kept true >>".to_vec();
    padding.resize(16 * MIB, b' ');
    let pdf = object_streams(&vec![padding; 4]);
    let error = vellum_engine::inspect(&pdf).expect_err("refused");
    assert!(
        error.contains("object streams may inflate to together"),
        "got {error}"
    );

    // Thirty-three million cross-reference entries of one byte each.
    let pdf = one_byte_xref_entries(33_000_000);
    let error = vellum_engine::inspect(&pdf).expect_err("refused");
    assert!(error.contains("more than 4000000 objects"), "got {error}");
    let error = vellum_engine::metadata(&pdf).expect_err("refused");
    assert!(error.contains("more than 4000000 objects"), "got {error}");

    let peak = peak_resident_bytes();
    assert!(
        peak < 768 * 1024 * 1024,
        "the process peaked at {} MiB",
        peak / (1024 * 1024)
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "took {:?}",
        started.elapsed()
    );

    // Under the ceilings, an object stream holding a large array still reads.
    let mut array = b"[".to_vec();
    array.extend(b"0 ".repeat(100_000));
    array.push(b']');
    let pdf = object_streams(&[array, b"<< /Kept true >>".to_vec()]);
    assert_eq!(vellum_engine::inspect(&pdf).expect("read").page_count, 1);
}
