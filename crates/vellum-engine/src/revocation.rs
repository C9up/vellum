//! Asking whether a certificate has been withdrawn, RFC 6960.
//!
//! A certificate can be valid on its face and worthless in fact: keys are
//! lost, employees leave, authorities discover a mistake. Only the issuer
//! knows, and only if asked — which means a network call, which this engine
//! does not make. So the work splits the way signing and timestamping already
//! do: build the question here, let the caller ask it, read the answer here.
//!
//! The answer has **three** values, not two. `Good` and `Revoked` are the
//! interesting ones; `Unknown` covers everything else — the responder was
//! unreachable, answered about a different certificate, or could not be
//! believed. Collapsing that third case into either of the others is the
//! mistake to avoid: treating it as good waves through a revoked certificate,
//! and treating it as revoked rejects documents whenever a server is down.
//! What to do about `Unknown` is the caller's policy, not this module's.

use const_oid::AssociatedOid;
use der::asn1::{BitString, GeneralizedTime, Null, OctetString};
use der::{Any, Choice, Decode, Encode, Enumerated, Sequence};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{AuthorityInfoAccessSyntax, ExtendedKeyUsage};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;

/// `id-ad-ocsp`, the access method that names a responder.
const ID_AD_OCSP: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1");
/// `id-pkix-ocsp-basic`, the only response type this reads.
const ID_PKIX_OCSP_BASIC: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1.1");
/// `id-kp-OCSPSigning`, which lets an authority delegate answering.
const ID_KP_OCSP_SIGNING: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.9");
/// SHA-1 is what RFC 6960 names a certificate with. It is not being used as a
/// security property here — the hashes identify, they do not authenticate —
/// and a responder keyed on anything else would not find the certificate.
const ID_SHA1: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.3.14.3.2.26");

/// Which certificate a question or an answer is about.
#[derive(Sequence, PartialEq, Eq)]
struct CertId {
    hash_algorithm: AlgorithmIdentifierOwned,
    issuer_name_hash: OctetString,
    issuer_key_hash: OctetString,
    serial_number: SerialNumber,
}

#[derive(Sequence)]
struct Request {
    req_cert: CertId,
}

#[derive(Sequence)]
struct TbsRequest {
    request_list: Vec<Request>,
}

#[derive(Sequence)]
struct OcspRequest {
    tbs_request: TbsRequest,
}

#[derive(Enumerated, Clone, Copy, PartialEq, Eq, Debug)]
#[asn1(type = "ENUMERATED")]
#[repr(u8)]
enum ResponseStatus {
    Successful = 0,
    MalformedRequest = 1,
    InternalError = 2,
    TryLater = 3,
    SigRequired = 5,
    Unauthorized = 6,
}

#[derive(Sequence)]
struct ResponseBytes {
    response_type: const_oid::ObjectIdentifier,
    response: OctetString,
}

#[derive(Sequence)]
struct OcspResponse {
    response_status: ResponseStatus,
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    response_bytes: Option<ResponseBytes>,
}

#[derive(Sequence)]
struct RevokedInfo {
    revocation_time: GeneralizedTime,
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    reason: Option<Any>,
}

#[derive(Choice)]
enum CertStatus {
    #[asn1(context_specific = "0", tag_mode = "IMPLICIT")]
    Good(Null),
    #[asn1(context_specific = "1", tag_mode = "IMPLICIT", constructed = "true")]
    Revoked(RevokedInfo),
    #[asn1(context_specific = "2", tag_mode = "IMPLICIT")]
    Unknown(Null),
}

#[derive(Sequence)]
struct SingleResponse {
    cert_id: CertId,
    cert_status: CertStatus,
    this_update: GeneralizedTime,
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    next_update: Option<GeneralizedTime>,
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT", optional = "true")]
    extensions: Option<Any>,
}

#[derive(Sequence)]
struct ResponseData {
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    version: Option<u8>,
    /// A CHOICE naming the responder. It is not read: the responder is
    /// identified by finding the certificate whose key actually verifies the
    /// answer, which is the thing that matters.
    responder_id: Any,
    produced_at: GeneralizedTime,
    responses: Vec<SingleResponse>,
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT", optional = "true")]
    extensions: Option<Any>,
}

#[derive(Sequence)]
struct BasicOcspResponse {
    tbs_response_data: ResponseData,
    signature_algorithm: AlgorithmIdentifierOwned,
    signature: BitString,
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT", optional = "true")]
    certs: Option<Vec<Certificate>>,
}

/// What the issuer says about a certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revocation {
    /// The issuer says it stands.
    Good,
    /// The issuer says it was withdrawn, at this instant.
    Revoked { at: String },
    /// Nobody could be believed about it. The reason is carried so a caller
    /// can decide what to do rather than guess.
    Unknown { reason: String },
}

impl Revocation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Revoked { .. } => "revoked",
            Self::Unknown { .. } => "unknown",
        }
    }
}

/// The responder a certificate names, if it names one.
pub fn responder_url(certificate: &[u8]) -> Option<String> {
    let certificate = Certificate::from_der(certificate).ok()?;
    let access: AuthorityInfoAccessSyntax = certificate
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id == AuthorityInfoAccessSyntax::OID)
        .and_then(|extension| {
            AuthorityInfoAccessSyntax::from_der(extension.extn_value.as_bytes()).ok()
        })?;

    access.0.iter().find_map(|description| {
        if description.access_method != ID_AD_OCSP {
            return None;
        }
        match &description.access_location {
            GeneralName::UniformResourceIdentifier(url) => Some(url.as_str().to_string()),
            _ => None,
        }
    })
}

/// Name a certificate the way a responder expects.
fn cert_id(certificate: &Certificate, issuer: &Certificate) -> Result<CertId, String> {
    let name = issuer
        .tbs_certificate
        .subject
        .to_der()
        .map_err(|error| format!("cannot read the issuer's name: {error}"))?;
    let key = issuer
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();

    Ok(CertId {
        hash_algorithm: AlgorithmIdentifierOwned {
            oid: ID_SHA1,
            parameters: Some(Any::null()),
        },
        issuer_name_hash: OctetString::new(Sha1::digest(&name).as_slice())
            .map_err(|error| format!("cannot encode the question: {error}"))?,
        issuer_key_hash: OctetString::new(Sha1::digest(key).as_slice())
            .map_err(|error| format!("cannot encode the question: {error}"))?,
        serial_number: certificate.tbs_certificate.serial_number.clone(),
    })
}

/// Build the question to post to a responder.
///
/// No nonce is sent. Responders routinely serve pre-produced answers, which
/// cannot echo one, and a client that demanded it would fail against most of
/// the internet. Freshness is judged from the answer's own dates instead.
pub fn revocation_query(certificate: &[u8], issuer: &[u8]) -> Result<Vec<u8>, String> {
    let certificate = Certificate::from_der(certificate)
        .map_err(|error| format!("cannot read the certificate: {error}"))?;
    let issuer = Certificate::from_der(issuer)
        .map_err(|error| format!("cannot read the issuer: {error}"))?;

    OcspRequest {
        tbs_request: TbsRequest {
            request_list: vec![Request {
                req_cert: cert_id(&certificate, &issuer)?,
            }],
        },
    }
    .to_der()
    .map_err(|error| format!("cannot encode the question: {error}"))
}

/// Whether a certificate may answer for the issuer.
fn may_answer(candidate: &Certificate, issuer: &Certificate) -> bool {
    if candidate == issuer {
        return true;
    }
    // A delegate has to be issued by the authority it answers for, and say
    // that answering is what it is for.
    if candidate.tbs_certificate.issuer != issuer.tbs_certificate.subject {
        return false;
    }
    candidate
        .tbs_certificate
        .extensions
        .as_ref()
        .and_then(|extensions| {
            extensions
                .iter()
                .find(|extension| extension.extn_id == ExtendedKeyUsage::OID)
        })
        .and_then(|extension| ExtendedKeyUsage::from_der(extension.extn_value.as_bytes()).ok())
        .is_some_and(|usage| usage.0.contains(&ID_KP_OCSP_SIGNING))
}

/// Verify that `signed` was signed by `candidate`, with the algorithm named.
fn answered_by(
    signed: &[u8],
    signature: &[u8],
    algorithm: &AlgorithmIdentifierOwned,
    candidate: &Certificate,
) -> bool {
    let Ok(spki) = candidate.tbs_certificate.subject_public_key_info.to_der() else {
        return false;
    };
    let Some(key) = rsa::pkcs8::SubjectPublicKeyInfoRef::from_der(&spki)
        .ok()
        .and_then(|info| rsa::RsaPublicKey::try_from(info).ok())
    else {
        return false;
    };
    let Ok(signature) = rsa::pkcs1v15::Signature::try_from(signature) else {
        return false;
    };

    match algorithm.oid.to_string().as_str() {
        "1.2.840.113549.1.1.11" => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key),
            signed,
            &signature,
        )
        .is_ok(),
        "1.2.840.113549.1.1.12" => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<sha2::Sha384>::new(key),
            signed,
            &signature,
        )
        .is_ok(),
        "1.2.840.113549.1.1.13" => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(key),
            signed,
            &signature,
        )
        .is_ok(),
        _ => false,
    }
}

/// Read a responder's answer about one certificate.
///
/// `at` is the instant the document was signed, which is what the answer has
/// to be about: a certificate revoked yesterday was perfectly good last year.
pub fn read_revocation(
    response: &[u8],
    certificate: &[u8],
    issuer: &[u8],
    at: Option<u64>,
) -> Revocation {
    let unknown = |reason: &str| Revocation::Unknown {
        reason: reason.to_string(),
    };

    let (Ok(certificate), Ok(issuer)) = (
        Certificate::from_der(certificate),
        Certificate::from_der(issuer),
    ) else {
        return unknown("the certificate or its issuer could not be read");
    };
    let Ok(asked) = cert_id(&certificate, &issuer) else {
        return unknown("the certificate could not be named");
    };

    let Ok(parsed) = OcspResponse::from_der(response) else {
        return unknown("the responder's answer could not be read");
    };
    if parsed.response_status != ResponseStatus::Successful {
        return Revocation::Unknown {
            reason: format!("the responder answered {:?}", parsed.response_status),
        };
    }
    let Some(bytes) = parsed.response_bytes else {
        return unknown("the responder answered nothing");
    };
    if bytes.response_type != ID_PKIX_OCSP_BASIC {
        return unknown("the responder answered in a form this cannot read");
    }
    let Ok(basic) = BasicOcspResponse::from_der(bytes.response.as_bytes()) else {
        return unknown("the responder's answer could not be read");
    };

    // The answer is only worth reading if the issuer, or someone it authorised,
    // actually signed it.
    let Ok(signed) = basic.tbs_response_data.to_der() else {
        return unknown("the responder's answer could not be re-encoded");
    };
    let Some(signature) = basic.signature.as_bytes() else {
        return unknown("the responder's answer is not signed");
    };
    let candidates: Vec<Certificate> = basic
        .certs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .chain(std::iter::once(issuer.clone()))
        .collect();
    let believed = candidates.iter().any(|candidate| {
        may_answer(candidate, &issuer)
            && answered_by(&signed, signature, &basic.signature_algorithm, candidate)
    });
    if !believed {
        return unknown("nobody entitled to answer for this issuer signed the answer");
    }

    let Some(single) = basic
        .tbs_response_data
        .responses
        .iter()
        .find(|single| single.cert_id == asked)
    else {
        return unknown("the responder answered about a different certificate");
    };

    // An answer that had already been superseded when the document was signed
    // says nothing about that moment.
    if let (Some(at), Some(next)) = (at, &single.next_update) {
        if next.to_unix_duration().as_secs() < at {
            return unknown("the responder's answer was already stale when this was signed");
        }
    }

    match &single.cert_status {
        CertStatus::Good(_) => Revocation::Good,
        CertStatus::Revoked(info) => {
            let when = info.revocation_time.to_unix_duration().as_secs();
            // Revoked after this document was signed does not taint it: that
            // is what a signing time, and better a timestamp, is for.
            match at {
                Some(at) if at < when => Revocation::Good,
                _ => Revocation::Revoked {
                    at: info.revocation_time.to_date_time().to_string(),
                },
            }
        }
        CertStatus::Unknown(_) => unknown("the issuer does not recognise this certificate"),
    }
}

#[cfg(test)]
mod tests {
    use der::Encode;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::RsaPrivateKey;

    use super::*;
    use crate::cms::tests::{authority_key, key_and_chain};

    const SIGNED_AT: u64 = 1_788_532_200; // 2026-09-04T14:30:00Z

    fn at(seconds: u64) -> GeneralizedTime {
        GeneralizedTime::from_unix_duration(std::time::Duration::from_secs(seconds))
            .expect("a date")
    }

    /// An answer, signed by whoever the test says answers.
    fn answer(status: CertStatus, key: &[u8], responder: Option<Certificate>) -> Vec<u8> {
        let (_, certificate, issuer) = key_and_chain();
        let certificate = Certificate::from_der(&certificate).expect("the certificate");
        let issuer_certificate = Certificate::from_der(&issuer).expect("the issuer");

        let data = ResponseData {
            version: None,
            // byName, which this does not read.
            responder_id: Any::from_der(
                &der::asn1::ContextSpecific {
                    tag_number: der::TagNumber::new(1),
                    tag_mode: der::TagMode::Explicit,
                    value: issuer_certificate.tbs_certificate.subject.clone(),
                }
                .to_der()
                .expect("a responder id"),
            )
            .expect("a responder id"),
            produced_at: at(SIGNED_AT),
            responses: vec![SingleResponse {
                cert_id: cert_id(&certificate, &issuer_certificate).expect("a cert id"),
                cert_status: status,
                this_update: at(SIGNED_AT - 3600),
                next_update: Some(at(SIGNED_AT + 86_400)),
                extensions: None,
            }],
            extensions: None,
        };

        let signed = data.to_der().expect("encodable");
        let private = RsaPrivateKey::from_pkcs8_der(key).expect("a key");
        let signature = SigningKey::<Sha256>::new(private).sign(&signed);

        let basic = BasicOcspResponse {
            tbs_response_data: data,
            signature_algorithm: AlgorithmIdentifierOwned {
                oid: const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11"),
                parameters: Some(Any::null()),
            },
            signature: BitString::new(0, signature.to_vec()).expect("a bit string"),
            certs: responder.map(|certificate| vec![certificate]),
        }
        .to_der()
        .expect("encodable");

        OcspResponse {
            response_status: ResponseStatus::Successful,
            response_bytes: Some(ResponseBytes {
                response_type: ID_PKIX_OCSP_BASIC,
                response: OctetString::new(basic).expect("an octet string"),
            }),
        }
        .to_der()
        .expect("encodable")
    }

    fn read(response: &[u8], at: Option<u64>) -> Revocation {
        let (_, certificate, issuer) = key_and_chain();
        read_revocation(response, &certificate, &issuer, at)
    }

    #[test]
    fn the_question_names_the_certificate_it_asks_about() {
        let (_, certificate, issuer) = key_and_chain();
        let query = revocation_query(&certificate, &issuer).expect("a query");

        let parsed = OcspRequest::from_der(&query).expect("it decodes");
        let asked = &parsed.tbs_request.request_list[0].req_cert;
        let expected = cert_id(
            &Certificate::from_der(&certificate).unwrap(),
            &Certificate::from_der(&issuer).unwrap(),
        )
        .expect("a cert id");
        assert!(asked == &expected, "the serial and issuer have to match");
    }

    #[test]
    fn finds_the_responder_the_certificate_names() {
        let (_, certificate, _) = key_and_chain();
        assert_eq!(
            responder_url(&certificate).as_deref(),
            Some("http://ocsp.vellum.test/")
        );
    }

    #[test]
    fn an_issuer_saying_it_stands_is_believed() {
        let response = answer(CertStatus::Good(Null), &authority_key(), None);
        assert_eq!(read(&response, Some(SIGNED_AT)), Revocation::Good);
    }

    #[test]
    fn an_issuer_saying_it_was_withdrawn_is_believed() {
        let response = answer(
            CertStatus::Revoked(RevokedInfo {
                revocation_time: at(SIGNED_AT - 86_400),
                reason: None,
            }),
            &authority_key(),
            None,
        );
        assert!(matches!(
            read(&response, Some(SIGNED_AT)),
            Revocation::Revoked { .. }
        ));
    }

    /// The nuance that makes a signing time worth recording: a certificate
    /// withdrawn after a document was signed does not taint that document.
    #[test]
    fn a_certificate_withdrawn_after_signing_does_not_taint_the_signature() {
        let response = answer(
            CertStatus::Revoked(RevokedInfo {
                revocation_time: at(SIGNED_AT + 86_400),
                reason: None,
            }),
            &authority_key(),
            None,
        );
        assert_eq!(read(&response, Some(SIGNED_AT)), Revocation::Good);
        // Without an instant there is nothing to compare against, so the
        // withdrawal stands.
        assert!(matches!(read(&response, None), Revocation::Revoked { .. }));
    }

    /// An answer signed by someone the issuer never authorised is not an
    /// answer. Believing it would let anyone revoke anything.
    #[test]
    fn an_answer_from_the_wrong_signer_is_not_believed() {
        let (signing_key, certificate, _) = key_and_chain();
        let response = answer(
            CertStatus::Good(Null),
            &signing_key,
            Some(Certificate::from_der(&certificate).expect("the certificate")),
        );

        match read(&response, Some(SIGNED_AT)) {
            Revocation::Unknown { reason } => {
                assert!(reason.contains("entitled"), "got {reason:?}")
            }
            other => panic!("it should not be believed, got {other:?}"),
        }
    }

    #[test]
    fn a_refusal_is_not_a_good_answer() {
        let response = OcspResponse {
            response_status: ResponseStatus::TryLater,
            response_bytes: None,
        }
        .to_der()
        .expect("encodable");

        match read(&response, Some(SIGNED_AT)) {
            Revocation::Unknown { reason } => assert!(reason.contains("TryLater")),
            other => panic!("a refusal is not an answer, got {other:?}"),
        }
    }

    #[test]
    fn nonsense_is_unknown_rather_than_good() {
        assert!(matches!(
            read(b"not an answer", Some(SIGNED_AT)),
            Revocation::Unknown { .. }
        ));
    }
}
