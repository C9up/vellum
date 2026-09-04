//! Checking the signatures already on a document.
//!
//! Signing is half the job. A document that arrives signed is worth nothing
//! until someone has checked that the signature covers it, that the bytes have
//! not moved since, and that the signature was made by the certificate it
//! claims.
//!
//! What this establishes is **integrity and authorship**, not **trust**. It
//! does not ask whether the certificate comes from an authority you accept,
//! nor whether it has since been revoked: that needs a trust store and a live
//! revocation check, neither of which belongs in a PDF engine. The report says
//! what was checked, and a caller that needs more has the certificate to check
//! it with.
//!
//! The trap this exists to catch is the one everybody meets first: a
//! `/ByteRange` that does not reach the end of the file. Content appended
//! after a signature is not covered by it, and a reader that only verifies the
//! arithmetic will happily call such a document signed.

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::{Decode, Encode, Reader};
use lopdf::{Document, Object};
use sha2::{Digest, Sha256};
use x509_cert::Certificate;

use crate::form::fields_of;
use crate::trust::{evaluate, read_anchors, Moment, TrustOptions};

/// `id-aa-signatureTimeStampToken`.
const SIGNATURE_TIME_STAMP: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");

/// What one signature on a document turns out to be.
#[derive(Debug, Clone)]
pub struct SignatureReport {
    /// The field the signature sits in.
    pub field: String,
    /// The signed byte range runs from the first byte to the last, so nothing
    /// was appended after the signature was made.
    pub covers_whole_document: bool,
    /// The document's bytes hash to what the signature committed to.
    pub digest_matches: bool,
    /// The signature verifies against the certificate it carries.
    pub signature_verifies: bool,
    /// Who the certificate says signed.
    pub signer: Option<String>,
    /// When the signature says it was made, as the signer stated it.
    pub signed_at: Option<String>,
    /// A timestamp authority has vouched for when — so the signature outlives
    /// the certificate's own validity.
    pub timestamped: bool,
    /// A path was found from the signer's certificate to a trusted anchor.
    /// False whenever no anchors were supplied, which is the honest answer.
    pub trusted: bool,
    /// That path, the signer first and the anchor last.
    pub chain: Vec<String>,
    /// Where the instant used to judge the path came from: a timestamp, the
    /// signer's own claim, or nowhere.
    pub moment: &'static str,
    /// That instant, in seconds since the epoch, for a caller who has to ask
    /// somebody else about it.
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
    anchors: &[Certificate],
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

    let mut hasher = Sha256::new();
    hasher.update(&pdf[start..first]);
    hasher.update(&pdf[second..end]);
    let digest: [u8; 32] = hasher.finalize().into();

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

    report.timestamped = signer.unsigned_attrs.as_ref().is_some_and(|attributes| {
        attributes
            .iter()
            .any(|attribute| attribute.oid == SIGNATURE_TIME_STAMP)
    });

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

    report.signed_at = attributes
        .iter()
        .find(|attribute| attribute.oid == const_oid::db::rfc5911::ID_SIGNING_TIME)
        .and_then(|attribute| attribute.values.as_slice().first())
        .and_then(|value| value.to_der().ok())
        .and_then(|der| cms::attr::SigningTime::from_der(&der).ok())
        .map(|time| match time {
            cms::attr::SigningTime::UtcTime(time) => time.to_date_time().to_string(),
            cms::attr::SigningTime::GeneralTime(time) => time.to_date_time().to_string(),
        });

    // The certificate the signature carries, matched to the signer it names.
    let certificate = signed.certificates.as_ref().and_then(|set| {
        set.0.iter().find_map(|choice| match choice {
            cms::cert::CertificateChoices::Certificate(certificate) => Some(certificate.clone()),
            _ => None,
        })
    });
    let Some(certificate) = certificate else {
        report
            .problems
            .push("the signature carries no certificate to check it against".to_string());
        return report;
    };
    report.signer = Some(subject_of(&certificate));

    // Judged when the document was signed, not now: a certificate that has
    // since expired did not retroactively unsign anything. A timestamp is
    // worth having because it makes that instant something other than the
    // signer's own word.
    let stamped = signer
        .unsigned_attrs
        .as_ref()
        .and_then(|attributes| {
            attributes
                .iter()
                .find(|attribute| attribute.oid == SIGNATURE_TIME_STAMP)
        })
        .and_then(|attribute| attribute.values.as_slice().first())
        .and_then(crate::timestamp::stamped_at);
    let claimed = attributes
        .iter()
        .find(|attribute| attribute.oid == const_oid::db::rfc5911::ID_SIGNING_TIME)
        .and_then(|attribute| attribute.values.as_slice().first())
        .and_then(|value| value.to_der().ok())
        .and_then(|der| cms::attr::SigningTime::from_der(&der).ok())
        .map(|time| match time {
            cms::attr::SigningTime::UtcTime(time) => time.to_unix_duration().as_secs(),
            cms::attr::SigningTime::GeneralTime(time) => time.to_unix_duration().as_secs(),
        });

    let (at, moment) = match (stamped, claimed) {
        (Some(at), _) => (Some(at), Moment::Timestamp),
        (None, Some(at)) => (Some(at), Moment::Claimed),
        (None, None) => (None, Moment::Unknown),
    };
    report.moment = moment.as_str();
    report.moment_at = at;
    report.signer_certificate = certificate.to_der().ok();

    let carried: Vec<Certificate> = signed
        .certificates
        .as_ref()
        .map(|set| {
            set.0
                .iter()
                .filter_map(|choice| match choice {
                    cms::cert::CertificateChoices::Certificate(certificate) => {
                        Some(certificate.clone())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let trust = evaluate(&certificate, &carried, anchors, at);
    report.trusted = trust.trusted;
    report.chain = trust.chain;
    report.issuer_certificate = trust.issuer;
    report.problems.extend(trust.problems);

    let verified = certificate
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .ok()
        .and_then(|spki| {
            let key = rsa::pkcs8::SubjectPublicKeyInfoRef::from_der(&spki).ok()?;
            rsa::RsaPublicKey::try_from(key).ok()
        })
        .and_then(|key| {
            let signed_bytes = attributes.to_der().ok()?;
            let signature = rsa::pkcs1v15::Signature::try_from(signer.signature.as_bytes()).ok()?;
            Some(
                rsa::signature::Verifier::verify(
                    &rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key),
                    &signed_bytes,
                    &signature,
                )
                .is_ok(),
            )
        });
    match verified {
        Some(true) => report.signature_verifies = true,
        Some(false) => report
            .problems
            .push("the signature does not match the certificate it carries".to_string()),
        None => report
            .problems
            .push("the certificate does not carry a key this can check with".to_string()),
    }

    report
}

/// Report on every signature the document carries, in field order.
///
/// A document with no signatures reports none; that is an answer, not a
/// failure.
pub fn verify_signatures(
    pdf: &[u8],
    options: &TrustOptions,
) -> Result<Vec<SignatureReport>, String> {
    let document = Document::load_mem(pdf).map_err(|error| format!("cannot read PDF: {error}"))?;

    let (anchors, anchor_problems) = read_anchors(&options.anchors);
    let mut reports = Vec::new();
    for (field_id, field) in fields_of(&document) {
        if field.kind != crate::FieldKind::Signature {
            continue;
        }
        let signature = document
            .get_dictionary(field_id)
            .ok()
            .and_then(|dictionary| dictionary.get(b"V").ok().cloned())
            .and_then(|value| match value {
                Object::Reference(id) => document.get_dictionary(id).ok().cloned(),
                Object::Dictionary(dictionary) => Some(dictionary),
                _ => None,
            });

        match signature {
            Some(signature) => {
                let mut report = verify_one(pdf, field.name, &signature, &anchors);
                report.problems.extend(anchor_problems.iter().cloned());
                reports.push(report);
            }
            // A signature field with no value is an empty place for one, not a
            // broken signature.
            None => continue,
        }
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

    /// A timestamp is what keeps a signature verifiable past its
    /// certificate's expiry, so whether one is there belongs in the report.
    #[test]
    fn a_timestamped_signature_is_reported_as_one() {
        assert!(
            !verify_signatures(&signed_document(), &TrustOptions::default()).expect("it reads")[0]
                .timestamped,
            "the plain one carries none"
        );

        let marked = create_blank(&[(595.28, 841.89)]).unwrap();
        let prepared = prepare(&marked, &SignatureOptions::default()).expect("preparing");
        let (key, certificate) = key_and_certificate();
        let value = sign_cms(
            &prepared.digest,
            &key,
            std::slice::from_ref(&certificate),
            "2026-09-04T14:30:00Z",
        )
        .expect("signing");

        let (_, nonce) = crate::timestamp_query(&value).expect("a query");
        let stamped = crate::attach_timestamp(
            &value,
            &crate::timestamp::tests::granted_response(&value, nonce),
            nonce,
        )
        .expect("attaching");

        let document = embed_signature(&prepared.document, &stamped).expect("embedding");
        let report = &verify_signatures(&document, &TrustOptions::default()).expect("it reads")[0];

        assert!(report.timestamped, "the timestamp should be reported");
        assert!(
            report.digest_matches && report.signature_verifies,
            "and attaching one must not have disturbed the signature: {:?}",
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

    /// A certificate is judged at the moment of signing, so one that was not
    /// yet valid then cannot be rescued by being valid now.
    #[test]
    fn a_certificate_not_valid_when_it_signed_is_reported() {
        let (key, certificate, authority) = key_and_chain();
        // The fixture is valid from 2020; this claims to have signed in 2019.
        let document = signed_at(&key, &certificate, "2019-06-01T10:00:00Z");

        let report = &verify_signatures(
            &document,
            &TrustOptions {
                anchors: vec![authority],
            },
        )
        .expect("it reads")[0];

        assert!(!report.trusted);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("not valid when the document was signed")),
            "got {:?}",
            report.problems
        );
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
