//! Checking the signatures already on a document.
//!
//! Signing is half the job. A document that arrives signed is worth nothing
//! until someone has checked that the signature covers it, that the bytes have
//! not moved since, and that the signature was made by the certificate it
//! names.
//!
//! Integrity and authorship are settled from the document alone. Trust is
//! settled against the anchors a caller supplies: the signer's certificate has
//! to chain to one of them, every link signed by the next, at the instant the
//! signature is judged at. Revocation needs a responder, which this engine
//! does not ask; the report carries the certificates to ask about.
//!
//! The trap this exists to catch is the one everybody meets first: a
//! `/ByteRange` that does not reach the end of the file. Content appended
//! after a signature is not covered by it, and a reader that only verifies the
//! arithmetic will happily call such a document signed.

use std::collections::HashMap;

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::{Decode, Encode, Reader};
use lopdf::xref::XrefEntry;
use lopdf::{Document, Object, ObjectId};
use sha2::{Digest, Sha256};
use x509_cert::Certificate;

use crate::form::fields_of;
use crate::trust::{
    carried, evaluate, named_certificate, read_anchors, rsa_verifies, Hash, Moment, TrustOptions,
};

/// `id-aa-signatureTimeStampToken`.
const SIGNATURE_TIME_STAMP: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");

/// What one signature on a document turns out to be.
#[derive(Debug, Clone)]
pub struct SignatureReport {
    /// The field the signature sits in.
    pub field: String,
    /// The signed byte range runs from the first byte to the last, so nothing
    /// was appended after the signature was made, and the one region it
    /// leaves out is this signature's own value.
    pub covers_whole_document: bool,
    /// The document's bytes hash to what the signature committed to.
    pub digest_matches: bool,
    /// The signature verifies against the certificate its signer names.
    pub signature_verifies: bool,
    /// Who the certificate says signed.
    pub signer: Option<String>,
    /// When the signature says it was made, as the signer stated it. Shown,
    /// never used to judge anything.
    pub signed_at: Option<String>,
    /// A timestamp token whose authority's signature checks out, and chains
    /// to an anchor, vouches for when — so the signature outlives the
    /// certificate's own validity.
    pub timestamped: bool,
    /// A path was found from the signer's certificate to a trusted anchor,
    /// every link valid at `moment_at`. False whenever no anchors were
    /// supplied, which is the honest answer.
    pub trusted: bool,
    /// That path, the signer first and the anchor last.
    pub chain: Vec<String>,
    /// What the signature offers about when it was made: a verified
    /// timestamp, only the signer's own claim, or nothing. Only a verified
    /// timestamp moves the instant the path is judged at.
    pub moment: &'static str,
    /// The instant the path was judged at, in seconds since the epoch: the
    /// timestamp's when `moment` is `timestamp`, the time of the check
    /// otherwise. It is the instant to ask a revocation responder about.
    pub moment_at: Option<u64>,
    /// The certificate that signed, so a caller can ask about it.
    pub signer_certificate: Option<Vec<u8>>,
    /// The certificate that issued it, which is who answers about revocation.
    pub issuer_certificate: Option<Vec<u8>>,
    /// Everything that could not be checked, or checked out wrong.
    pub problems: Vec<String>,
}

impl SignatureReport {
    fn failed(field: String, problem: String) -> Self {
        Self {
            field,
            covers_whole_document: false,
            digest_matches: false,
            signature_verifies: false,
            signer: None,
            signed_at: None,
            timestamped: false,
            trusted: false,
            chain: Vec::new(),
            moment: Moment::Unknown.as_str(),
            moment_at: None,
            signer_certificate: None,
            issuer_certificate: None,
            problems: vec![problem],
        }
    }
}

/// The four numbers of a `/ByteRange`.
fn byte_range(signature: &lopdf::Dictionary) -> Option<[usize; 4]> {
    let values = signature.get(b"ByteRange").ok()?.as_array().ok()?;
    let mut range = [0usize; 4];
    if values.len() != 4 {
        return None;
    }
    for (slot, value) in range.iter_mut().zip(values) {
        *slot = usize::try_from(value.as_i64().ok()?).ok()?;
    }
    Some(range)
}

/// Whether the region a byte range leaves out is exactly the `/Contents`
/// value of the signature, as ISO 32000-2 §12.8.1 requires.
///
/// The value is reserved larger than it needs and padded, and the padding is
/// unsigned. Without this check it could be rewritten — closing the
/// signature dictionary early, with a /Reason and /Name of anyone's choosing
/// — while every signed byte stayed the same. `own_value` is where the
/// signature dictionary's own `/Contents` value sits in the file: a
/// `/Contents` with the same digits in another object, or nested one level
/// down, would otherwise let the range leave out a region of its choosing.
fn gap_is_value(
    pdf: &[u8],
    first: usize,
    second: usize,
    value: &[u8],
    own_value: Option<(usize, usize)>,
) -> bool {
    if own_value != Some((first, second)) {
        return false;
    }
    let Some([b'<', inner @ .., b'>']) = pdf.get(first..second) else {
        return false;
    };
    let mut digits = inner.iter().filter(|byte| !byte.is_ascii_whitespace());
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(&high) = digits.next() {
        // An odd digit count ends as if followed by a 0 (§7.3.4.3).
        let low = digits.next().copied().unwrap_or(b'0');
        let (Some(high), Some(low)) = (hex_value(high), hex_value(low)) else {
            return false;
        };
        decoded.push(high << 4 | low);
    }
    decoded == value
}

/// Where the signature dictionary `id` has its own `/Contents` value in the
/// file: the bytes from its `<` to its `>`.
///
/// Read from the offset the cross-reference table gives the object, entry by
/// entry at the dictionary's top level, so a `/Contents` nested inside one of
/// its values does not count. `None` when the object is not written there as
/// `id generation obj << ... >>`, sits in an object stream (where no byte of
/// it can be left out of a range), or names `/Contents` other than once.
fn own_contents_span(pdf: &[u8], document: &Document, id: ObjectId) -> Option<(usize, usize)> {
    let Some(XrefEntry::Normal { offset, generation }) = document.reference_table.get(id.0) else {
        return None;
    };
    if *generation != id.1 {
        return None;
    }
    let mut lexer = Lexer {
        bytes: pdf,
        at: usize::try_from(*offset).ok()?,
    };
    lexer.skip_space();
    (lexer.integer()? == u64::from(id.0)).then_some(())?;
    lexer.skip_space();
    (lexer.integer()? == u64::from(id.1)).then_some(())?;
    lexer.skip_space();
    lexer.keyword(b"obj")?;
    lexer.skip_space();
    lexer.tag(b"<<")?;
    let mut span = None;
    loop {
        lexer.skip_space();
        if lexer.tag(b">>").is_some() {
            return span;
        }
        let key = lexer.name()?;
        lexer.skip_space();
        let start = lexer.at;
        lexer.value()?;
        if key == b"Contents" {
            if span.is_some() {
                return None;
            }
            span = Some((start, lexer.at));
        }
    }
}

/// How deep the values of a signature dictionary may nest before reading it
/// gives up, the bound the parser that read the document applies too.
const MAX_VALUE_DEPTH: usize = 512;

/// The tokens of ISO 32000-2 §7.2-7.3, enough to step over one value.
struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.at + ahead).copied()
    }

    fn skip_space(&mut self) {
        while let Some(byte) = self.peek(0) {
            if is_white(byte) {
                self.at += 1;
            } else if byte == b'%' {
                while self
                    .peek(0)
                    .is_some_and(|byte| byte != b'\n' && byte != b'\r')
                {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    fn tag(&mut self, tag: &[u8]) -> Option<()> {
        self.bytes.get(self.at..)?.starts_with(tag).then(|| {
            self.at += tag.len();
        })
    }

    /// A keyword, which a regular character would continue into another.
    fn keyword(&mut self, keyword: &[u8]) -> Option<()> {
        let start = self.at;
        self.tag(keyword)?;
        if self.peek(0).is_some_and(is_regular) {
            self.at = start;
            return None;
        }
        Some(())
    }

    fn integer(&mut self) -> Option<u64> {
        let start = self.at;
        let mut value: u64 = 0;
        while let Some(digit) = self.peek(0).filter(u8::is_ascii_digit) {
            value = value
                .checked_mul(10)?
                .checked_add(u64::from(digit - b'0'))?;
            self.at += 1;
        }
        if self.at == start || self.peek(0).is_some_and(is_regular) {
            self.at = start;
            return None;
        }
        Some(value)
    }

    /// A name, its `#xx` escapes decoded (§7.3.5).
    fn name(&mut self) -> Option<Vec<u8>> {
        self.tag(b"/")?;
        let mut name = Vec::new();
        while let Some(byte) = self.peek(0).filter(|&byte| is_regular(byte)) {
            let escaped = (byte == b'#')
                .then(|| Some(hex_value(self.peek(1)?)? << 4 | hex_value(self.peek(2)?)?))
                .flatten();
            match escaped {
                Some(decoded) => {
                    name.push(decoded);
                    self.at += 3;
                }
                None => {
                    name.push(byte);
                    self.at += 1;
                }
            }
        }
        Some(name)
    }

    fn literal_string(&mut self) -> Option<()> {
        self.tag(b"(")?;
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek(0)? {
                b'\\' => self.at += 1,
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            self.at += 1;
        }
        Some(())
    }

    fn hex_string(&mut self) -> Option<()> {
        self.tag(b"<")?;
        let close = self
            .bytes
            .get(self.at..)?
            .iter()
            .position(|&byte| byte == b'>')?;
        self.at += close + 1;
        Some(())
    }

    fn regular(&mut self) -> Option<()> {
        let start = self.at;
        while self.peek(0).is_some_and(is_regular) {
            self.at += 1;
        }
        (self.at > start).then_some(())
    }

    /// One value: a string, a name, a number, a keyword, a reference, or an
    /// array or dictionary with everything inside it.
    fn value(&mut self) -> Option<()> {
        match (self.peek(0)?, self.peek(1)) {
            (b'[', _) | (b'<', Some(b'<')) => self.composite(),
            (b'<', _) => self.hex_string(),
            (b'(', _) => self.literal_string(),
            (b'/', _) => self.name().map(|_| ()),
            _ => {
                if self.integer().is_some() {
                    // `id generation R` is one value.
                    let after = self.at;
                    self.skip_space();
                    let reference = self.integer().is_some() && {
                        self.skip_space();
                        self.keyword(b"R").is_some()
                    };
                    if !reference {
                        self.at = after;
                    }
                    return Some(());
                }
                self.regular()
            }
        }
    }

    /// An array or a dictionary, each closer matched against its opener.
    fn composite(&mut self) -> Option<()> {
        let mut closers: Vec<&[u8]> = Vec::new();
        loop {
            self.skip_space();
            match (self.peek(0)?, self.peek(1)) {
                (b'[', _) => {
                    self.at += 1;
                    closers.push(b"]");
                }
                (b'<', Some(b'<')) => {
                    self.at += 2;
                    closers.push(b">>");
                }
                (b']', _) | (b'>', Some(b'>')) => {
                    let closer = closers.pop()?;
                    self.tag(closer)?;
                    if closers.is_empty() {
                        return Some(());
                    }
                    continue;
                }
                (b'<', _) => self.hex_string()?,
                (b'(', _) => self.literal_string()?,
                (b'/', _) => self.name().map(|_| ())?,
                _ => self.regular()?,
            }
            if closers.len() > MAX_VALUE_DEPTH {
                return None;
            }
        }
    }
}

fn is_white(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_regular(byte: u8) -> bool {
    !is_white(byte) && !b"()<>[]{}/%".contains(&byte)
}

fn hex_value(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// Take the first DER object out of a buffer, ignoring the padding after it.
///
/// The signature was written into a fixed reservation, so what follows it is
/// whatever the reservation was filled with.
fn first_object(bytes: &[u8]) -> Result<&[u8], String> {
    let mut reader = der::SliceReader::new(bytes)
        .map_err(|error| format!("the signature value is not readable: {error}"))?;
    reader
        .tlv_bytes()
        .map_err(|error| format!("the signature value is not readable: {error}"))
}

/// The subject of a certificate, as a reader would show it.
fn subject_of(certificate: &Certificate) -> String {
    certificate.tbs_certificate.subject.to_string()
}

/// Verify one signature, reporting rather than refusing: a caller wants to
/// know what is wrong with a document, not merely that something is.
fn verify_one(
    pdf: &[u8],
    field: String,
    signature: &lopdf::Dictionary,
    own_value: Option<(usize, usize)>,
    anchors: &[Certificate],
    now: u64,
    digests: &mut HashMap<[usize; 4], [u8; 32]>,
) -> SignatureReport {
    let Some(range) = byte_range(signature) else {
        return SignatureReport::failed(field, "the signature declares no byte range".to_string());
    };
    let Ok(Object::String(value, _)) = signature.get(b"Contents") else {
        return SignatureReport::failed(field, "the signature carries no value".to_string());
    };

    let [start, first, second, length] = range;
    let end = second.saturating_add(length);
    let within = start == 0
        && first <= pdf.len()
        && second <= pdf.len()
        && end <= pdf.len()
        && second >= first;
    if !within {
        return SignatureReport::failed(
            field,
            "the signed byte range does not fit the document".to_string(),
        );
    }

    let mut report = SignatureReport {
        field,
        // The whole point: anything appended after the signature is outside
        // it, and a document is only signed if the range reaches its last byte.
        covers_whole_document: end == pdf.len(),
        digest_matches: false,
        signature_verifies: false,
        signer: None,
        signed_at: None,
        timestamped: false,
        trusted: false,
        chain: Vec::new(),
        moment: Moment::Unknown.as_str(),
        moment_at: None,
        signer_certificate: None,
        issuer_certificate: None,
        problems: Vec::new(),
    };
    if !report.covers_whole_document {
        report.problems.push(format!(
            "{} bytes were added after this signature and are not covered by it",
            pdf.len() - end
        ));
    }
    if !gap_is_value(pdf, first, second, value, own_value) {
        report.covers_whole_document = false;
        report.problems.push(
            "the unsigned region is not this signature's value, so bytes outside it are not covered"
                .to_string(),
        );
    }

    // Signatures sharing a byte range share its digest: hashing it once per
    // signature let thousands of them over one large range cost minutes.
    let digest = *digests.entry(range).or_insert_with(|| {
        let mut hasher = Sha256::new();
        hasher.update(&pdf[start..first]);
        hasher.update(&pdf[second..end]);
        hasher.finalize().into()
    });

    let cms = match first_object(value).and_then(|der| {
        ContentInfo::from_der(der)
            .map_err(|error| format!("the signature is not readable: {error}"))
    }) {
        Ok(content) => content,
        Err(problem) => {
            report.problems.push(problem);
            return report;
        }
    };
    let signed: SignedData = match cms.content.decode_as() {
        Ok(signed) => signed,
        Err(error) => {
            report
                .problems
                .push(format!("the signature is not readable: {error}"));
            return report;
        }
    };

    let Some(signer) = signed.signer_infos.0.as_slice().first() else {
        report
            .problems
            .push("the signature has no signer".to_string());
        return report;
    };

    let Some(attributes) = signer.signed_attrs.as_ref() else {
        report
            .problems
            .push("the signature carries no signed attributes".to_string());
        return report;
    };

    // What the signature committed to, next to what the document actually is.
    let committed = attributes
        .iter()
        .find(|attribute| attribute.oid == const_oid::db::rfc5911::ID_MESSAGE_DIGEST)
        .and_then(|attribute| attribute.values.as_slice().first())
        .and_then(|value| value.to_der().ok())
        .and_then(|der| der::asn1::OctetString::from_der(&der).ok())
        .map(|octets| octets.as_bytes().to_vec());
    match committed {
        Some(committed) => {
            report.digest_matches = committed == digest;
            if !report.digest_matches {
                report
                    .problems
                    .push("the document has changed since it was signed".to_string());
            }
        }
        None => report
            .problems
            .push("the signature commits to no digest".to_string()),
    }

    let claimed = attributes
        .iter()
        .find(|attribute| attribute.oid == const_oid::db::rfc5911::ID_SIGNING_TIME)
        .and_then(|attribute| attribute.values.as_slice().first())
        .and_then(|value| value.to_der().ok())
        .and_then(|der| cms::attr::SigningTime::from_der(&der).ok());
    report.signed_at = claimed.as_ref().map(|time| match time {
        cms::attr::SigningTime::UtcTime(time) => time.to_date_time().to_string(),
        cms::attr::SigningTime::GeneralTime(time) => time.to_date_time().to_string(),
    });

    // The certificate the signer names, by issuer and serial or by key
    // identifier, among those the signature carries: the first of the set is
    // whichever the document put first.
    let carried = carried(signed.certificates.as_ref());
    let Some(certificate) = named_certificate(&signer.sid, &carried).cloned() else {
        report.problems.push(if carried.is_empty() {
            "the signature carries no certificate to check it against".to_string()
        } else {
            "the signature carries no certificate for the signer it names".to_string()
        });
        return report;
    };
    report.signer = Some(subject_of(&certificate));
    report.signer_certificate = certificate.to_der().ok();
    let names_its_certificate = binds_certificate(attributes, &certificate);
    if let Err(problem) = &names_its_certificate {
        report.problems.push(problem.clone());
    }

    // The instant the path is judged at: now, unless a timestamp token that
    // checks out proves the signature existed earlier. The token is an
    // unsigned attribute, so it is verified before anything in it is read;
    // the signer's own signingTime never moves the instant.
    let token = signer
        .unsigned_attrs
        .as_ref()
        .and_then(|attributes| {
            attributes
                .iter()
                .find(|attribute| attribute.oid == SIGNATURE_TIME_STAMP)
        })
        .and_then(|attribute| attribute.values.as_slice().first());
    let stamped = token.map(|token| {
        crate::timestamp::verify_token(token, signer.signature.as_bytes(), anchors, now)
    });
    let (at, moment) = match stamped {
        Some(Ok(at)) => (at, Moment::Timestamp),
        Some(Err(problem)) => {
            report.problems.push(format!(
                "the timestamp cannot be believed, so the signature is judged at the current time: {problem}"
            ));
            (
                now,
                if claimed.is_some() {
                    Moment::Claimed
                } else {
                    Moment::Unknown
                },
            )
        }
        None if claimed.is_some() => (now, Moment::Claimed),
        None => (now, Moment::Unknown),
    };
    report.timestamped = moment == Moment::Timestamp;
    report.moment = moment.as_str();
    report.moment_at = Some(at);

    let trust = evaluate(&certificate, &carried, anchors, at);
    report.trusted = trust.trusted && names_its_certificate.is_ok();
    report.chain = trust.chain;
    report.issuer_certificate = trust.issuer;
    report.problems.extend(trust.problems);

    let verified = attributes
        .to_der()
        .map_err(|error| format!("the signed attributes cannot be re-encoded: {error}"))
        .and_then(|signed_bytes| {
            let hash = Hash::of_signature(
                &signer.signature_algorithm.oid,
                Some(&signer.digest_alg.oid),
            )
            .ok_or_else(|| "the signature uses an algorithm this cannot check".to_string())?;
            rsa_verifies(
                &certificate.tbs_certificate.subject_public_key_info,
                hash,
                &signed_bytes,
                signer.signature.as_bytes(),
            )
        });
    match verified {
        Ok(true) => report.signature_verifies = true,
        Ok(false) => report
            .problems
            .push("the signature does not match the certificate it carries".to_string()),
        Err(problem) => report.problems.push(format!(
            "the signature cannot be checked against its certificate: {problem}"
        )),
    }

    report
}

/// `id-aa-signingCertificateV2`, RFC 5035 §3.
const SIGNING_CERTIFICATE_V2: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");

/// RFC 5035 §4: one certificate by its hash, the digest `DEFAULT id-sha256`.
#[derive(der::Sequence)]
struct EssCertIdV2 {
    #[asn1(optional = "true")]
    hash_algorithm: Option<x509_cert::spki::AlgorithmIdentifierOwned>,
    cert_hash: der::asn1::OctetString,
    #[asn1(optional = "true")]
    issuer_serial: Option<der::Any>,
}

#[derive(der::Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
    #[asn1(optional = "true")]
    policies: Option<der::Any>,
}

/// Whether the signed `signing-certificate-v2` attribute, when there is one,
/// names `certificate`.
///
/// The attribute is what binds a signature to a certificate rather than to a
/// key: a second certificate for the same key, carried in the signature,
/// would otherwise be reported as the signer.
fn binds_certificate(
    attributes: &x509_cert::attr::Attributes,
    certificate: &Certificate,
) -> Result<(), String> {
    let Some(attribute) = attributes
        .iter()
        .find(|attribute| attribute.oid == SIGNING_CERTIFICATE_V2)
    else {
        return Ok(());
    };
    let named = attribute
        .values
        .as_slice()
        .first()
        .and_then(|value| value.to_der().ok())
        .and_then(|der| SigningCertificateV2::from_der(&der).ok())
        .ok_or_else(|| {
            "the signature's signing-certificate attribute is not readable".to_string()
        })?;
    let first = named
        .certs
        .first()
        .ok_or_else(|| "the signature's signing-certificate attribute names nothing".to_string())?;
    let hash = match &first.hash_algorithm {
        None => Hash::Sha256,
        Some(algorithm) => Hash::of_digest(&algorithm.oid).ok_or_else(|| {
            "the signature names its certificate with a digest this cannot check".to_string()
        })?,
    };
    let encoded = certificate
        .to_der()
        .map_err(|error| format!("cannot re-encode the signer's certificate: {error}"))?;
    if hash.digest(&encoded) != first.cert_hash.as_bytes() {
        return Err(
            "the signature names another certificate than the one reported as its signer"
                .to_string(),
        );
    }
    Ok(())
}

/// How many distinct signatures one call checks. Each costs a hash of up to
/// the whole file and a certificate path; real documents carry a handful.
const MAX_SIGNATURES: usize = 100;

/// Report on every signature the document carries, in field order.
///
/// A document with no signatures reports none; that is an answer, not a
/// failure.
pub fn verify_signatures(
    pdf: &[u8],
    options: &TrustOptions,
) -> Result<Vec<SignatureReport>, String> {
    verify_signatures_at(pdf, options, crate::trust::now())
}

/// {@link verify_signatures} judged at `now`, so a test can stand at a stated
/// instant.
pub(crate) fn verify_signatures_at(
    pdf: &[u8],
    options: &TrustOptions,
    now: u64,
) -> Result<Vec<SignatureReport>, String> {
    let document = Document::load_mem(pdf).map_err(|error| format!("cannot read PDF: {error}"))?;

    let (anchors, anchor_problems) = read_anchors(&options.anchors);
    let mut reports = Vec::new();
    let mut digests = HashMap::new();
    // Fields naming one signature dictionary share its report.
    let mut checked: HashMap<ObjectId, SignatureReport> = HashMap::new();
    let mut distinct = 0;
    for (field_id, field) in fields_of(&document)? {
        if field.kind != crate::FieldKind::Signature {
            continue;
        }
        let value = document
            .get_dictionary(field_id)
            .ok()
            .and_then(|dictionary| dictionary.get(b"V").ok().cloned());
        let shared = match &value {
            Some(Object::Reference(id)) => Some(*id),
            _ => None,
        };
        if let Some(report) = shared.and_then(|id| checked.get(&id)) {
            reports.push(SignatureReport {
                field: field.name,
                ..report.clone()
            });
            continue;
        }
        let signature = value.and_then(|value| match value {
            Object::Reference(id) => document.get_dictionary(id).ok().cloned(),
            Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        });
        // A signature field with no value is an empty place for one, not a
        // broken signature.
        let Some(signature) = signature else {
            continue;
        };

        distinct += 1;
        if distinct > MAX_SIGNATURES {
            reports.push(SignatureReport::failed(
                field.name,
                format!("not checked: the document carries more than {MAX_SIGNATURES} signatures"),
            ));
            continue;
        }
        // Only an indirect signature dictionary has an object of its own to
        // hold the value: a field's /V shall be an indirect reference
        // (ISO 32000-2 Table 255).
        let own_value = shared.and_then(|id| own_contents_span(pdf, &document, id));
        let mut report = verify_one(
            pdf,
            field.name,
            &signature,
            own_value,
            &anchors,
            now,
            &mut digests,
        );
        report.problems.extend(anchor_problems.iter().cloned());
        if let Some(id) = shared {
            checked.insert(id, report.clone());
        }
        reports.push(report);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cms::tests::{key_and_certificate, key_and_chain};
    use crate::{
        create_blank, embed_signature, prepare, sign_cms, stamp_text, SignatureOptions,
        TextStampOptions,
    };

    /// A document with something written on it, signed the way an application
    /// would sign it: prepare, sign the digest, put the value in.
    fn signed_document() -> Vec<u8> {
        let marked = stamp_text(
            &create_blank(&[(595.28, 841.89)]).unwrap(),
            "BROUILLON",
            &TextStampOptions {
                x: 60.0,
                y: 300.0,
                size: 36.0,
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        let prepared = prepare(
            &marked,
            &SignatureOptions {
                name: Some("Amelie Durand".to_string()),
                signed_at: Some("2026-09-04T14:30:00Z".to_string()),
                ..Default::default()
            },
        )
        .expect("preparing should succeed");

        let (key, certificate) = key_and_certificate();
        let value = sign_cms(
            &prepared.digest,
            &key,
            std::slice::from_ref(&certificate),
            "2026-09-04T14:30:00Z",
        )
        .expect("signing should succeed");

        embed_signature(&prepared.document, &value).expect("embedding should succeed")
    }

    fn document_signed_with(key: &[u8], certificate: &[u8]) -> Vec<u8> {
        signed_at(key, certificate, "2026-09-04T14:30:00Z")
    }

    fn signed_at(key: &[u8], certificate: &[u8], when: &str) -> Vec<u8> {
        let prepared = prepare(
            &create_blank(&[(595.28, 841.89)]).unwrap(),
            &SignatureOptions {
                signed_at: Some(when.to_string()),
                ..Default::default()
            },
        )
        .expect("preparing should succeed");
        let value = sign_cms(
            &prepared.digest,
            key,
            std::slice::from_ref(&certificate.to_vec()),
            when,
        )
        .expect("signing should succeed");
        embed_signature(&prepared.document, &value).expect("embedding should succeed")
    }

    /// Base64 for the PEM test, the other way round from the decoder.
    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut buffer = [0u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(char::from(
                        ALPHABET[((value >> (18 - index * 6)) & 0x3F) as usize],
                    ));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn a_signed_document_checks_out() {
        let reports =
            verify_signatures(&signed_document(), &TrustOptions::default()).expect("it reads");
        assert_eq!(reports.len(), 1);
        let report = &reports[0];

        assert!(report.covers_whole_document, "{:?}", report.problems);
        assert!(report.digest_matches, "{:?}", report.problems);
        assert!(report.signature_verifies, "{:?}", report.problems);
        // Integrity is settled; trust is a separate question and, with no
        // anchors supplied, its answer is no.
        assert!(!report.trusted);
        assert_eq!(
            report.problems,
            vec!["no trusted anchors were supplied, so nothing can be trusted"]
        );
        assert!(
            report
                .signer
                .as_deref()
                .is_some_and(|signer| signer.contains("Vellum Test")),
            "the certificate's subject should be reported, got {:?}",
            report.signer
        );
        assert!(report.signed_at.is_some());
        assert!(!report.timestamped, "nothing timestamped this one");
    }

    /// The signed byte range of a document's only signature.
    fn gap_of(pdf: &[u8]) -> (usize, usize) {
        let document = Document::load_mem(pdf).expect("it reads");
        let signature = document
            .objects
            .values()
            .filter_map(|object| object.as_dict().ok())
            .find(|dictionary| dictionary.get(b"ByteRange").is_ok())
            .expect("a signature dictionary");
        let [_, first, second, _] = byte_range(signature).expect("a byte range");
        (first, second)
    }

    /// The reservation for the value is mostly padding, outside the signed
    /// bytes. Rewritten to close the dictionary early, it hands the signer's
    /// /Reason and /Name to whoever edits it, without touching a signed byte.
    #[test]
    fn rewriting_the_unsigned_gap_is_caught() {
        let mut forged = signed_document();
        let (first, second) = gap_of(&forged);
        // The value's own digits, without the zero padding after them.
        let hex_digits = forged[first + 1..second - 1]
            .iter()
            .rposition(|byte| *byte != b'0')
            .map_or(0, |at| at + 1);
        let mut gap = forged[first..first + 1 + hex_digits].to_vec();
        gap.extend_from_slice(b"> /Reason (FORGED: amount changed) /Name (Mallory) >>\nendobj\n%");
        gap.resize(second - first, b' ');
        forged[first..second].copy_from_slice(&gap);

        let reports = verify_signatures(&forged, &TrustOptions::default()).expect("it reads");
        let report = &reports[0];
        assert!(!report.covers_whole_document, "{:?}", report.problems);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("not this signature's value")),
            "{:?}",
            report.problems
        );
    }

    /// Where the byte range is written before its numbers are known.
    const RANGE: &str = "[0 0000000000 0000000000 0000000000]";

    /// The value every signature dictionary below carries.
    const VALUE: &str = "<C0FFEE>";

    /// A one-page document whose form holds one signature field, numbered
    /// from 4 on (`objects`), with every `RANGE` placeholder set to exclude
    /// the first `VALUE` after `gap_after`.
    fn document_with_gap(objects: &[&str], gap_after: &str) -> Vec<u8> {
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /SigFlags 3 >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
        ];
        bodies.extend_from_slice(objects);
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
        }
        let xref_at = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                bodies.len() + 1
            )
            .as_bytes(),
        );

        let marker = out
            .windows(gap_after.len())
            .position(|window| window == gap_after.as_bytes())
            .expect("the marker is written");
        let first = marker
            + out[marker..]
                .windows(VALUE.len())
                .position(|window| window == VALUE.as_bytes())
                .expect("the value follows the marker");
        let second = first + VALUE.len();
        let range = format!("[0 {first:010} {second:010} {:010}]", out.len() - second);
        while let Some(at) = out
            .windows(RANGE.len())
            .position(|window| window == RANGE.as_bytes())
        {
            out[at..at + RANGE.len()].copy_from_slice(range.as_bytes());
        }
        out
    }

    const FIELD: &str = "<< /FT /Sig /T (Signature1) /V 5 0 R >>";

    fn unsigned_region_is_the_value(pdf: &[u8]) -> bool {
        let report = &verify_signatures(pdf, &TrustOptions::default()).expect("it reads")[0];
        let flagged = report
            .problems
            .iter()
            .any(|problem| problem.contains("not this signature's value"));
        assert_eq!(
            report.covers_whole_document, !flagged,
            "{:?}",
            report.problems
        );
        report.covers_whole_document
    }

    #[test]
    fn the_unsigned_region_is_the_value_of_the_signature_dictionary_itself() {
        let signature = format!("<< /Type /Sig /ByteRange {RANGE} /Contents <C0FFEE> >>");
        let pdf = document_with_gap(&[FIELD, &signature], "5 0 obj");
        assert!(unsigned_region_is_the_value(&pdf));
    }

    /// A key may be spelt with `#xx` escapes (§7.3.5); `/Cont#65nts` is
    /// `/Contents`, and its value is the signature's.
    #[test]
    fn an_escaped_contents_key_still_owns_the_value() {
        let signature = format!("<< /Type /Sig /ByteRange {RANGE} /Cont#65nts <C0FFEE> >>");
        let pdf = document_with_gap(&[FIELD, &signature], "5 0 obj");
        assert!(unsigned_region_is_the_value(&pdf));
    }

    /// Another object carrying a `/Contents` with the same digits is not the
    /// signature's value, and the region it fills is not the signature's to
    /// leave out.
    #[test]
    fn a_contents_in_another_object_is_not_the_signatures_value() {
        let signature = format!("<< /Type /Sig /ByteRange {RANGE} /Contents <C0FFEE> >>");
        let decoy = "<< /Contents <C0FFEE> >>";
        let pdf = document_with_gap(&[FIELD, &signature, decoy], "6 0 obj");
        assert!(!unsigned_region_is_the_value(&pdf));
    }

    /// Nor is a `/Contents` nested inside the signature dictionary, one level
    /// down: only the dictionary's own entry is its value.
    #[test]
    fn a_contents_nested_inside_the_signature_is_not_its_value() {
        let signature = format!(
            "<< /Type /Sig /ByteRange {RANGE} /Prop_Build << /Contents <C0FFEE> >> /Contents <C0FFEE> >>"
        );
        let pdf = document_with_gap(&[FIELD, &signature], "/Prop_Build <<");
        assert!(!unsigned_region_is_the_value(&pdf));
    }

    /// Text that reads `/Contents <...>` inside a string is not an entry.
    #[test]
    fn a_contents_written_inside_a_string_is_not_the_value() {
        let signature = format!(
            "<< /Type /Sig /ByteRange {RANGE} /Reason (see /Contents {VALUE}) /Contents {VALUE} >>"
        );
        let pdf = document_with_gap(&[FIELD, &signature], "/Reason (");
        assert!(!unsigned_region_is_the_value(&pdf));
    }

    /// A signature field's value shall be an indirect reference (ISO 32000-2
    /// Table 255): written inline, it has no object of its own to own the
    /// region it leaves out.
    #[test]
    fn a_signature_written_inline_in_its_field_is_not_located() {
        let field = format!(
            "<< /FT /Sig /T (Signature1) /V << /Type /Sig /ByteRange {RANGE} /Contents <C0FFEE> >> >>"
        );
        let pdf = document_with_gap(&[&field], "4 0 obj");
        assert!(!unsigned_region_is_the_value(&pdf));
    }

    /// Two `/Contents` entries leave which one is the value to whichever
    /// reader is asked; neither is taken.
    #[test]
    fn a_signature_with_two_contents_entries_is_not_located() {
        let signature =
            format!("<< /Type /Sig /Contents <C0FFEE> /ByteRange {RANGE} /Contents <C0FFEE> >>");
        let pdf = document_with_gap(&[FIELD, &signature], "/ByteRange");
        assert!(!unsigned_region_is_the_value(&pdf));
    }

    /// The trap everyone meets first. Content appended after a signature is
    /// not covered by it, and a document with a valid signature over its first
    /// half is not a signed document.
    #[test]
    fn appending_to_a_signed_document_is_caught() {
        let mut tampered = signed_document();
        tampered.extend_from_slice(b"\n% and then someone added this\n");

        let reports = verify_signatures(&tampered, &TrustOptions::default()).expect("it reads");
        let report = &reports[0];

        assert!(
            !report.covers_whole_document,
            "the added bytes are outside the signature"
        );
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("added after")),
            "and the report has to say so, got {:?}",
            report.problems
        );
        // The arithmetic over the covered range still checks out, which is
        // exactly why coverage has to be reported separately.
        assert!(report.digest_matches);
        assert!(report.signature_verifies);
    }

    /// A byte changed inside the signed range breaks the digest.
    #[test]
    fn changing_the_document_is_caught() {
        let signed = signed_document();
        let at = signed
            .windows(9)
            .position(|window| window == b"BROUILLON")
            .expect("the fixture writes this");
        let mut tampered = signed.clone();
        // Same length, so nothing moves and only the digest can tell.
        tampered[at + 8] = b'M';

        let reports = verify_signatures(&tampered, &TrustOptions::default()).expect("it reads");
        let report = &reports[0];

        assert!(report.covers_whole_document, "nothing was added");
        assert!(!report.digest_matches, "but the bytes are not the same");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("changed since it was signed")),
            "got {:?}",
            report.problems
        );
    }

    /// A signature on a blank page by `key` and `certificate`, with a
    /// timestamp token at `gen_time` signed by `authority` (or by nobody)
    /// attached the way `attach_timestamp` attaches one.
    fn stamped_document(
        key: &[u8],
        certificate: &[u8],
        authority: Option<(&[u8], &[u8])>,
        gen_time: u64,
    ) -> Vec<u8> {
        let prepared = prepare(
            &create_blank(&[(595.28, 841.89)]).unwrap(),
            &SignatureOptions::default(),
        )
        .expect("preparing");
        let value = sign_cms(
            &prepared.digest,
            key,
            std::slice::from_ref(&certificate.to_vec()),
            "2026-09-04T14:30:00Z",
        )
        .expect("signing");
        let (_, nonce) = crate::timestamp_query(&value).expect("a query");
        let token = crate::timestamp::tests::token(
            &crate::timestamp::tests::imprint_of(&value),
            nonce,
            gen_time,
            authority,
        );
        let stamped =
            crate::attach_timestamp(&value, &crate::timestamp::tests::respond(0, token), nonce)
                .expect("attaching");
        embed_signature(&prepared.document, &stamped).expect("embedding")
    }

    fn anchored() -> TrustOptions {
        let (_, _, authority) = key_and_chain();
        TrustOptions {
            anchors: vec![authority],
        }
    }

    /// 2026-01-01 to 2026-10-01: valid when the test authority stamps
    /// (2026-09-04), lapsed by the time anybody runs this.
    fn lapsed_signer() -> (Vec<u8>, Vec<u8>) {
        static LAPSED: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
        LAPSED
            .get_or_init(|| {
                let key = crate::cms::tests::pkcs8(&crate::cms::tests::fresh_key());
                let certificate = crate::cms::tests::issued(
                    "CN=Vellum Lapsed Signer,O=Vellum",
                    6,
                    crate::cms::tests::spki_of(&key),
                    crate::cms::tests::window(1_767_225_600, 1_790_812_800),
                    None,
                    |_| {},
                );
                (key, certificate)
            })
            .clone()
    }

    /// A timestamp is what keeps a signature verifiable past its
    /// certificate's expiry, so whether one is there belongs in the report.
    #[test]
    fn a_timestamped_signature_is_reported_as_one() {
        assert!(
            !verify_signatures(&signed_document(), &anchored()).expect("it reads")[0].timestamped,
            "the plain one carries none"
        );

        let (key, certificate) = key_and_certificate();
        let (tsa_key, tsa) = crate::cms::tests::timestamp_authority();
        let document = stamped_document(
            &key,
            &certificate,
            Some((&tsa_key, &tsa)),
            crate::timestamp::tests::STAMPED_AT,
        );
        let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];

        assert!(report.timestamped, "{:?}", report.problems);
        assert_eq!(report.moment, "timestamp");
        assert_eq!(report.moment_at, Some(crate::timestamp::tests::STAMPED_AT));
        assert!(
            report.digest_matches && report.signature_verifies && report.trusted,
            "and attaching one must not have disturbed the signature: {:?}",
            report.problems
        );
    }

    /// The verified timestamp is what lets a lapsed certificate's signature
    /// stand: it proves the signature existed while the certificate was valid.
    #[test]
    fn a_verified_timestamp_judges_a_lapsed_certificate_when_it_stamped() {
        let (key, certificate) = lapsed_signer();
        let (tsa_key, tsa) = crate::cms::tests::timestamp_authority();
        let document = stamped_document(
            &key,
            &certificate,
            Some((&tsa_key, &tsa)),
            crate::timestamp::tests::STAMPED_AT,
        );
        let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];
        assert!(report.trusted, "{:?}", report.problems);
        assert_eq!(report.moment_at, Some(crate::timestamp::tests::STAMPED_AT));
    }

    /// The token sits outside what the signature covers. A hand-made one,
    /// dated inside the lapsed certificate's window, is not a timestamp, and
    /// the signature is judged now: lapsed, so untrusted.
    #[test]
    fn a_forged_timestamp_does_not_move_the_instant() {
        let (key, certificate) = lapsed_signer();
        let forged = stamped_document(
            &key,
            &certificate,
            None,
            crate::timestamp::tests::STAMPED_AT,
        );
        let now = crate::trust::now();
        let report = &verify_signatures_at(&forged, &anchored(), now).expect("it reads")[0];

        assert!(!report.timestamped);
        assert!(!report.trusted);
        assert_eq!(report.moment, "claimed");
        assert_eq!(report.moment_at, Some(now));
        let problems = report.problems.join(" | ");
        assert!(
            problems.contains("timestamp cannot be believed"),
            "{problems}"
        );
        assert!(problems.contains("was not valid"), "{problems}");
    }

    /// Signed by a timestamp authority of the forger's own making.
    #[test]
    fn a_timestamp_from_an_untrusted_authority_does_not_move_the_instant() {
        let (key, certificate) = lapsed_signer();
        let own_key = crate::cms::tests::pkcs8(&crate::cms::tests::fresh_key());
        let own = crate::cms::tests::issued_by(
            &own_key,
            "CN=Vellum Test Timestamps,O=Vellum",
            "CN=Vellum Test Timestamps,O=Vellum",
            5,
            crate::cms::tests::spki_of(&own_key),
            crate::cms::tests::window(1_577_836_800, 2_051_222_400),
            None,
            |builder| {
                builder
                    .add_extension(&x509_cert::ext::pkix::ExtendedKeyUsage(vec![
                        const_oid::ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.8"),
                    ]))
                    .unwrap();
            },
        );
        let forged = stamped_document(
            &key,
            &certificate,
            Some((&own_key, &own)),
            crate::timestamp::tests::STAMPED_AT,
        );
        let report = &verify_signatures(&forged, &anchored()).expect("it reads")[0];
        assert!(
            !report.timestamped && !report.trusted,
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn a_document_with_no_signature_reports_none() {
        let reports = verify_signatures(
            &create_blank(&[(595.28, 841.89)]).unwrap(),
            &TrustOptions::default(),
        )
        .expect("it reads");
        assert!(
            reports.is_empty(),
            "no signatures is an answer, not a fault"
        );
    }

    /// An unsigned signature field is a place for a signature, not a broken
    /// one, and reporting it as a failure would cry wolf.
    #[test]
    fn an_empty_signature_field_is_not_a_failure() {
        let reports = verify_signatures(
            &crate::fill::tests::form_document(),
            &TrustOptions::default(),
        )
        .expect("it reads");
        assert!(reports.is_empty());
    }

    /// A document signed under an authority the caller accepts. The path runs
    /// from the signing certificate up to that authority, and every link of it
    /// is checked.
    #[test]
    fn a_certificate_under_a_trusted_authority_is_trusted() {
        let (key, certificate, authority) = key_and_chain();
        let document = document_signed_with(&key, &certificate);

        let report = &verify_signatures(
            &document,
            &TrustOptions {
                anchors: vec![authority],
            },
        )
        .expect("it reads")[0];

        assert!(report.trusted, "{:?}", report.problems);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(
            report.chain.len(),
            2,
            "signer then authority: {:?}",
            report.chain
        );
        assert!(report.chain[0].contains("Signer"));
        assert!(report.chain[1].contains("Authority"));
        assert_eq!(
            report.moment, "claimed",
            "nothing timestamped it, so the instant is the signer's own word"
        );
    }

    /// Someone else's authority says nothing about this signature.
    #[test]
    fn an_unrelated_anchor_does_not_confer_trust() {
        let (key, certificate, _) = key_and_chain();
        let unrelated = crate::cms::tests::unrelated_authority();
        let document = document_signed_with(&key, &certificate);

        let report = &verify_signatures(
            &document,
            &TrustOptions {
                anchors: vec![unrelated],
            },
        )
        .expect("it reads")[0];

        assert!(!report.trusted);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("no path to a trusted anchor")),
            "got {:?}",
            report.problems
        );
    }

    /// The signer's own signingTime is its word, not proof. A certificate
    /// that has lapsed signs today, writing a date inside its old window: it
    /// is judged now, and is not trusted.
    #[test]
    fn a_claimed_signing_time_does_not_rescue_a_lapsed_certificate() {
        let (key, certificate) = lapsed_signer();
        let document = signed_at(&key, &certificate, "2026-06-01T10:00:00Z");
        let now = crate::trust::now();
        let report = &verify_signatures_at(&document, &anchored(), now).expect("it reads")[0];

        assert!(report.signature_verifies && report.digest_matches);
        assert!(!report.trusted, "{:?}", report.problems);
        assert_eq!(report.moment, "claimed");
        assert_eq!(report.moment_at, Some(now), "judged at the current time");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("was not valid")),
            "got {:?}",
            report.problems
        );
    }

    /// Nor does a claim make a valid certificate invalid: it is not read.
    #[test]
    fn a_claimed_signing_time_outside_the_window_is_not_what_is_judged() {
        let (key, certificate, _) = key_and_chain();
        // The fixture is valid from 2020; this claims to have signed in 2019.
        let document = signed_at(&key, &certificate, "2019-06-01T10:00:00Z");
        let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];
        assert!(report.trusted, "{:?}", report.problems);
        assert_eq!(report.moment, "claimed");
    }

    /// With nothing saying when, the certificate is judged now as well, not
    /// left unjudged.
    #[test]
    fn a_lapsed_certificate_with_no_time_at_all_is_not_trusted() {
        let (key, certificate) = lapsed_signer();
        let prepared = prepare(
            &create_blank(&[(595.28, 841.89)]).unwrap(),
            &SignatureOptions::default(),
        )
        .expect("preparing");
        let value = sign_cms(
            &prepared.digest,
            &key,
            std::slice::from_ref(&certificate),
            "2026-06-01T10:00:00Z",
        )
        .expect("signing");
        let value = without_signing_time(&value, &key);
        let document = embed_signature(&prepared.document, &value).expect("embedding");
        let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];
        assert_eq!(report.moment, "unknown");
        assert!(report.signature_verifies, "{:?}", report.problems);
        assert!(!report.trusted, "{:?}", report.problems);
    }

    /// The CMS with its signingTime taken out and the attributes signed again.
    fn without_signing_time(cms: &[u8], key: &[u8]) -> Vec<u8> {
        use rsa::signature::{SignatureEncoding, Signer};
        let content = ContentInfo::from_der(cms).unwrap();
        let mut signed: SignedData = content.content.decode_as().unwrap();
        let mut signers = signed.signer_infos.0.into_vec();
        let signer = signers.first_mut().unwrap();
        let kept: Vec<x509_cert::attr::Attribute> = signer
            .signed_attrs
            .as_ref()
            .unwrap()
            .iter()
            .filter(|attribute| attribute.oid != const_oid::db::rfc5911::ID_SIGNING_TIME)
            .cloned()
            .collect();
        let attributes = der::asn1::SetOfVec::try_from(kept).unwrap();
        let private =
            <rsa::RsaPrivateKey as rsa::pkcs8::DecodePrivateKey>::from_pkcs8_der(key).unwrap();
        let signature = rsa::pkcs1v15::SigningKey::<Sha256>::new(private)
            .sign(&attributes.to_der().unwrap())
            .to_vec();
        signer.signed_attrs = Some(attributes);
        signer.signature = der::asn1::OctetString::new(signature).unwrap();
        signed.signer_infos =
            cms::signed_data::SignerInfos(der::asn1::SetOfVec::try_from(signers).unwrap());
        ContentInfo {
            content_type: const_oid::db::rfc5911::ID_SIGNED_DATA,
            content: der::Any::encode_from(&signed).unwrap(),
        }
        .to_der()
        .unwrap()
    }

    /// The subject and serial number of a published root are public. A
    /// certificate copying them, with a key of its own, is not the root.
    #[test]
    fn a_certificate_cloning_an_anchors_name_and_serial_is_not_trusted() {
        let (key, clone) = crate::cms::tests::authority_clone();
        let document = document_signed_with(&key, &clone);
        let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];

        assert!(report.signature_verifies, "it verifies with its own key");
        assert!(!report.trusted, "{:?}", report.chain);
        assert_eq!(report.chain.len(), 1, "{:?}", report.chain);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("signed itself and is not one")),
            "got {:?}",
            report.problems
        );
    }

    /// The same clone, carried next to a genuine leaf as the "issuer" of a
    /// certificate it signed, does not lend the leaf the anchor's trust.
    #[test]
    fn a_leaf_issued_by_a_clone_of_the_anchor_is_not_trusted() {
        let (clone_key, _) = crate::cms::tests::authority_clone();
        let key = crate::cms::tests::pkcs8(&crate::cms::tests::fresh_key());
        let leaf = crate::cms::tests::issued_by(
            &clone_key,
            "CN=Vellum Test Authority,O=Vellum",
            "CN=Mallory,O=Vellum",
            8,
            crate::cms::tests::spki_of(&key),
            crate::cms::tests::window(1_577_836_800, 2_051_222_400),
            None,
            |_| {},
        );
        let document = document_signed_with(&key, &leaf);
        let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];
        assert!(!report.trusted, "{:?}", report.problems);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("not signed by the issuer")),
            "got {:?}",
            report.problems
        );
    }

    /// A trust anchor is a name and a key: the authority's certificate
    /// re-issued with the same name and key anchors the same paths.
    #[test]
    fn an_anchor_is_its_name_and_key() {
        let authority_key = crate::cms::tests::authority_key();
        let reissued = crate::cms::tests::issued_by(
            &authority_key,
            "CN=Vellum Test Authority,O=Vellum",
            "CN=Vellum Test Authority,O=Vellum",
            99,
            crate::cms::tests::spki_of(&authority_key),
            crate::cms::tests::window(1_577_836_800, 2_051_222_400),
            Some(None),
            |_| {},
        );
        let (key, certificate) = key_and_certificate();
        let document = document_signed_with(&key, &certificate);
        let report = &verify_signatures(
            &document,
            &TrustOptions {
                anchors: vec![reissued],
            },
        )
        .expect("it reads")[0];
        assert!(report.trusted, "{:?}", report.problems);
    }

    /// A second certificate for the same key, issued under another name.
    fn decoy() -> Vec<u8> {
        let (key, _) = key_and_certificate();
        crate::cms::tests::issued(
            "CN=Vellum Decoy,O=Vellum",
            7,
            crate::cms::tests::spki_of(&key),
            crate::cms::tests::window(1_577_836_800, 2_051_222_400),
            None,
            |_| {},
        )
    }

    fn signed_with_certificates(certificates: &[Vec<u8>]) -> Vec<u8> {
        let (key, _) = key_and_certificate();
        let prepared = prepare(
            &create_blank(&[(595.28, 841.89)]).unwrap(),
            &SignatureOptions::default(),
        )
        .expect("preparing");
        let value = sign_cms(&prepared.digest, &key, certificates, "2026-09-04T14:30:00Z")
            .expect("signing");
        embed_signature(&prepared.document, &value).expect("embedding")
    }

    /// The certificates of a CMS are a SET, ordered by their encoding, so
    /// whichever sorts first is the document's choice. The report is about
    /// the certificate the signer names, in either order.
    #[test]
    fn the_report_is_about_the_certificate_the_signer_names() {
        let (_, certificate) = key_and_certificate();
        let decoy = decoy();
        for (named, other, subject) in [
            (&certificate, &decoy, "Vellum Test Signer"),
            (&decoy, &certificate, "Vellum Decoy"),
        ] {
            let document = signed_with_certificates(&[named.clone(), other.clone()]);
            let report = &verify_signatures(&document, &anchored()).expect("it reads")[0];
            assert!(
                report
                    .signer
                    .as_deref()
                    .is_some_and(|signer| signer.contains(subject)),
                "expected {subject}, got {:?}",
                report.signer
            );
            assert_eq!(report.signer_certificate.as_ref(), Some(named));
            assert!(
                report.signature_verifies && report.trusted,
                "{:?}",
                report.problems
            );
        }
    }

    /// signing-certificate-v2 binds the signature to one certificate; any
    /// other certificate for the same key is refused.
    #[test]
    fn the_signing_certificate_attribute_has_to_name_the_signer() {
        let (_, certificate) = key_and_certificate();
        let document = signed_with_certificates(std::slice::from_ref(&certificate));
        let reports = verify_signatures(&document, &anchored()).expect("it reads");
        assert!(reports[0].trusted, "{:?}", reports[0].problems);

        let value = {
            let parsed = Document::load_mem(&document).unwrap();
            let signature = parsed
                .objects
                .values()
                .filter_map(|object| object.as_dict().ok())
                .find(|dictionary| dictionary.get(b"ByteRange").is_ok())
                .unwrap();
            match signature.get(b"Contents").unwrap() {
                Object::String(value, _) => first_object(value).unwrap().to_vec(),
                _ => panic!("a value"),
            }
        };
        let signed: SignedData = ContentInfo::from_der(&value)
            .unwrap()
            .content
            .decode_as()
            .unwrap();
        let attributes = signed.signer_infos.0.as_slice()[0]
            .signed_attrs
            .clone()
            .unwrap();
        assert!(
            binds_certificate(&attributes, &Certificate::from_der(&certificate).unwrap()).is_ok()
        );
        let error = binds_certificate(&attributes, &Certificate::from_der(&decoy()).unwrap())
            .expect_err("the decoy is not the certificate it names");
        assert!(error.contains("another certificate"), "got {error:?}");
    }

    /// PEM is what an authority usually publishes, so it has to be accepted
    /// without the caller converting it first.
    #[test]
    fn an_anchor_may_be_pem() {
        let (key, certificate, authority) = key_and_chain();
        let document = document_signed_with(&key, &certificate);

        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for chunk in base64_encode(&authority).as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(chunk).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");

        let report = &verify_signatures(
            &document,
            &TrustOptions {
                anchors: vec![pem.into_bytes()],
            },
        )
        .expect("it reads")[0];

        assert!(report.trusted, "{:?}", report.problems);
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(verify_signatures(b"not a PDF", &TrustOptions::default()).is_err());
    }
}
