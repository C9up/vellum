//! Deciding whether a certificate can be believed.
//!
//! Checking that a signature matches the certificate it carries says nothing
//! about who that certificate belongs to: anyone can make one. Trust comes
//! from a path to an anchor the caller has decided to accept — the trusted
//! list of a supervisory body, a company's own authority, whatever they choose
//! to supply. This module walks that path and checks every link of it.
//!
//! Two things it deliberately does not do. It does not fetch anything, so
//! **revocation is not checked**: a certificate withdrawn after it was issued
//! still looks valid here, and a caller who needs to know must ask OCSP or a
//! CRL. And it does not decide which anchors deserve trust — that is a policy
//! question, and a package that shipped its own answer would be making it for
//! everybody.
//!
//! The instant matters. A certificate valid when a document was signed expires
//! later, which does not retroactively unsign anything — so the path is judged
//! at the moment of signing, and the report says where that moment came from.
//! A timestamp is worth having precisely because it makes that moment
//! something other than the signer's word.

use der::{Decode, Encode};
use sha2::{Sha256, Sha384, Sha512};
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage, KeyUsages};
use x509_cert::Certificate;

/// How far a path may run before this gives up. Real chains are two or three
/// links; anything longer is a loop or an attempt at one.
const MAX_DEPTH: usize = 8;

/// What a caller is willing to believe.
#[derive(Debug, Clone, Default)]
pub struct TrustOptions {
    /// Certificates to trust as roots, DER or PEM. Empty means trust nothing,
    /// and every report comes back untrusted — which is the honest answer.
    pub anchors: Vec<Vec<u8>>,
}

/// Where the instant used to judge the path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// A timestamp authority's word.
    Timestamp,
    /// The signer's own claim, which is signed but not independent.
    Claimed,
    /// Nothing said when, so validity windows could not be judged.
    Unknown,
}

impl Moment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timestamp => "timestamp",
            Self::Claimed => "claimed",
            Self::Unknown => "unknown",
        }
    }
}

/// What walking the path found.
#[derive(Debug, Clone, Default)]
pub struct TrustReport {
    /// A path was found to one of the anchors, and every link checked out.
    pub trusted: bool,
    /// The subjects of the path, the signer first and the anchor last.
    pub chain: Vec<String>,
    pub problems: Vec<String>,
}

/// Read anchors, accepting either DER or PEM so a caller can paste what their
/// authority published without converting it first.
pub fn read_anchors(anchors: &[Vec<u8>]) -> (Vec<Certificate>, Vec<String>) {
    let mut read = Vec::new();
    let mut problems = Vec::new();

    for (index, bytes) in anchors.iter().enumerate() {
        let mut any = false;
        for der in split_pem(bytes) {
            match Certificate::from_der(&der) {
                Ok(certificate) => {
                    read.push(certificate);
                    any = true;
                }
                Err(error) => problems.push(format!("anchor {index} is not readable: {error}")),
            }
        }
        if !any && problems.is_empty() {
            problems.push(format!("anchor {index} holds no certificate"));
        }
    }
    (read, problems)
}

/// Every certificate in a buffer: the DER as it stands, or each PEM block.
fn split_pem(bytes: &[u8]) -> Vec<Vec<u8>> {
    const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END: &[u8] = b"-----END CERTIFICATE-----";

    if !bytes.windows(BEGIN.len()).any(|window| window == BEGIN) {
        return vec![bytes.to_vec()];
    }

    let mut out = Vec::new();
    let mut rest = bytes;
    while let Some(start) = rest.windows(BEGIN.len()).position(|w| w == BEGIN) {
        let body = &rest[start + BEGIN.len()..];
        let Some(end) = body.windows(END.len()).position(|w| w == END) else {
            break;
        };
        let base64: String = body[..end]
            .iter()
            .filter(|byte| !byte.is_ascii_whitespace())
            .map(|byte| char::from(*byte))
            .collect();
        if let Ok(der) = base64_decode(&base64) {
            out.push(der);
        }
        rest = &body[end + END.len()..];
    }
    out
}

/// Base64, without pulling in a crate for forty lines.
fn base64_decode(text: &str) -> Result<Vec<u8>, ()> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;

    for character in text.bytes() {
        if character == b'=' {
            break;
        }
        let Some(value) = ALPHABET.iter().position(|entry| *entry == character) else {
            return Err(());
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

/// Verify that `child` was signed by `issuer`.
fn signed_by(child: &Certificate, issuer: &Certificate) -> Result<(), String> {
    let tbs = child
        .tbs_certificate
        .to_der()
        .map_err(|error| format!("cannot re-encode a certificate: {error}"))?;
    let signature = child
        .signature
        .as_bytes()
        .ok_or_else(|| "a certificate's signature is malformed".to_string())?;

    let spki = issuer
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .map_err(|error| format!("cannot read an issuer's key: {error}"))?;
    let key = rsa::pkcs8::SubjectPublicKeyInfoRef::from_der(&spki)
        .ok()
        .and_then(|info| rsa::RsaPublicKey::try_from(info).ok())
        .ok_or_else(|| "an issuer's key is not RSA, which this cannot check with".to_string())?;

    let signature = rsa::pkcs1v15::Signature::try_from(signature)
        .map_err(|_| "a certificate's signature is malformed".to_string())?;
    let algorithm = child.signature_algorithm.oid;

    // Only the algorithms actually used by certificates that sign documents.
    // An unknown one is reported rather than assumed away.
    let verified = match algorithm.to_string().as_str() {
        "1.2.840.113549.1.1.11" => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key),
            &tbs,
            &signature,
        ),
        "1.2.840.113549.1.1.12" => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha384>::new(key),
            &tbs,
            &signature,
        ),
        "1.2.840.113549.1.1.13" => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha512>::new(key),
            &tbs,
            &signature,
        ),
        other => {
            return Err(format!(
                "a certificate is signed with {other}, which this cannot check"
            ))
        }
    };

    verified.map_err(|_| "a certificate is not signed by the issuer it names".to_string())
}

/// Seconds since the epoch for a certificate's validity bounds.
fn window(certificate: &Certificate) -> (u64, u64) {
    (
        certificate
            .tbs_certificate
            .validity
            .not_before
            .to_unix_duration()
            .as_secs(),
        certificate
            .tbs_certificate
            .validity
            .not_after
            .to_unix_duration()
            .as_secs(),
    )
}

fn extension<'a, T: der::Decode<'a> + const_oid::AssociatedOid>(
    certificate: &'a Certificate,
) -> Option<T> {
    certificate
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id == T::OID)
        .and_then(|extension| T::from_der(extension.extn_value.as_bytes()).ok())
}

/// Walk from `signer` to an anchor, checking every link.
///
/// `at` is the instant the path is judged at; when it is unknown the links are
/// still checked but the validity windows are not, and the report says so.
pub fn evaluate(
    signer: &Certificate,
    carried: &[Certificate],
    anchors: &[Certificate],
    at: Option<u64>,
) -> TrustReport {
    let mut report = TrustReport {
        chain: vec![signer.tbs_certificate.subject.to_string()],
        ..Default::default()
    };

    if anchors.is_empty() {
        report
            .problems
            .push("no trusted anchors were supplied, so nothing can be trusted".to_string());
        return report;
    }

    // The leaf has to be allowed to sign in the first place.
    if let Some(usage) = extension::<KeyUsage>(signer) {
        let allowed = usage.0.into_iter().any(|flag| {
            matches!(
                flag,
                KeyUsages::DigitalSignature | KeyUsages::NonRepudiation
            )
        });
        if !allowed {
            report
                .problems
                .push("the signer's certificate is not allowed to sign".to_string());
        }
    }

    let mut current = signer.clone();
    for depth in 0..MAX_DEPTH {
        if let Some(at) = at {
            let (from, until) = window(&current);
            if at < from || at > until {
                report.problems.push(format!(
                    "{} was not valid when the document was signed",
                    current.tbs_certificate.subject
                ));
            }
        }

        // An anchor ends the walk: it is trusted because the caller said so,
        // not because something above it vouches for it.
        let anchor = anchors.iter().find(|anchor| {
            anchor.tbs_certificate.subject == current.tbs_certificate.subject
                && anchor.tbs_certificate.serial_number == current.tbs_certificate.serial_number
        });
        if anchor.is_some() {
            report.trusted = report.problems.is_empty();
            return report;
        }

        let issuer = anchors
            .iter()
            .chain(carried.iter())
            .find(|candidate| candidate.tbs_certificate.subject == current.tbs_certificate.issuer)
            .cloned();
        let Some(issuer) = issuer else {
            report.problems.push(format!(
                "no path to a trusted anchor: nothing here issued {}",
                current.tbs_certificate.issuer
            ));
            return report;
        };

        if let Err(problem) = signed_by(&current, &issuer) {
            report.problems.push(problem);
            return report;
        }

        // Anything that issues certificates has to say that it does.
        if depth > 0 || issuer.tbs_certificate.subject != current.tbs_certificate.subject {
            let is_authority = extension::<BasicConstraints>(&issuer).is_some_and(|basic| basic.ca);
            if !is_authority {
                report.problems.push(format!(
                    "{} signed a certificate without being an authority",
                    issuer.tbs_certificate.subject
                ));
            }
        }

        report
            .chain
            .push(issuer.tbs_certificate.subject.to_string());
        current = issuer;
    }

    report
        .problems
        .push("the certificate path is too long to be genuine".to_string());
    report
}
