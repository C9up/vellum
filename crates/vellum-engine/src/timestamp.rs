//! Timestamping a signature, RFC 3161.
//!
//! A signature proves a document has not changed since a key signed it. It
//! does not prove *when*, and that matters more than it sounds: once the
//! signing certificate expires, a verifier has no way to tell a signature made
//! while it was valid from one forged afterwards, and stops accepting it. A
//! trusted timestamp is what keeps a signature verifiable for the decades a
//! contract is kept.
//!
//! The authority is asked over HTTP, which this engine does not do. So the
//! work is split the same way signing is: build the query here, let the caller
//! post it, attach the answer here. What comes back is a token from a third
//! party, so it is checked rather than trusted — the status, and that the
//! authority stamped the imprint we actually sent.

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use const_oid::ObjectIdentifier;
use der::asn1::{OctetString, SetOfVec};
use der::{Any, Decode, Encode, Reader, Sequence};
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::spki::AlgorithmIdentifierOwned;

/// `id-aa-signatureTimeStampToken`, RFC 3161 §3.3.
const SIGNATURE_TIME_STAMP: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");
const SHA_256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");

/// What the authority is asked to stamp.
#[derive(Sequence)]
struct MessageImprint {
    hash_algorithm: AlgorithmIdentifierOwned,
    hashed_message: OctetString,
}

/// RFC 3161 §2.4.1. `reqPolicy` and `extensions` are left out; `certReq` is
/// set, so the authority returns its certificate — a verifier that has to go
/// and find it usually cannot.
#[derive(Sequence)]
struct TimeStampReq {
    version: u8,
    message_imprint: MessageImprint,
    #[asn1(optional = "true")]
    nonce: Option<u64>,
    cert_req: bool,
}

/// RFC 2510 §3.2.3, as RFC 3161 uses it.
#[derive(Sequence)]
struct PkiStatusInfo {
    status: u32,
    #[asn1(optional = "true")]
    status_string: Option<Vec<String>>,
}

#[derive(Sequence)]
struct TimeStampResp {
    status: PkiStatusInfo,
    #[asn1(optional = "true")]
    token: Option<Any>,
}

fn sha256() -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid: SHA_256,
        parameters: None,
    }
}

/// The signature value inside a CMS, which is what a signature timestamp is
/// taken over.
fn signature_of(cms: &[u8]) -> Result<(SignedData, Vec<u8>), String> {
    let content = ContentInfo::from_der(cms)
        .map_err(|error| format!("cannot read the signature: {error}"))?;
    let signed: SignedData = content
        .content
        .decode_as()
        .map_err(|error| format!("cannot read the signature: {error}"))?;

    let signer = signed
        .signer_infos
        .0
        .as_slice()
        .first()
        .ok_or_else(|| "the signature has no signer".to_string())?;
    let value = signer.signature.as_bytes().to_vec();
    Ok((signed, value))
}

/// Build the query to post to a timestamp authority.
///
/// The nonce is returned with it: the authority has to echo it, and checking
/// that is what stops an old answer being replayed as a fresh one.
pub fn timestamp_query(cms: &[u8]) -> Result<(Vec<u8>, u64), String> {
    let (_, signature) = signature_of(cms)?;
    let imprint = Sha256::digest(&signature);

    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|error| format!("cannot draw a nonce: {error}"))?;
    // Cleared so the value always encodes as a positive INTEGER.
    let nonce = u64::from_be_bytes(bytes) >> 1;

    let request = TimeStampReq {
        version: 1,
        message_imprint: MessageImprint {
            hash_algorithm: sha256(),
            hashed_message: OctetString::new(imprint.as_slice())
                .map_err(|error| format!("cannot encode the imprint: {error}"))?,
        },
        nonce: Some(nonce),
        cert_req: true,
    };

    let encoded = request
        .to_der()
        .map_err(|error| format!("cannot encode the timestamp query: {error}"))?;
    Ok((encoded, nonce))
}

/// Read what the authority sent back, refusing anything but a granted answer.
fn token_of(response: &[u8]) -> Result<Any, String> {
    let parsed = TimeStampResp::from_der(response)
        .map_err(|error| format!("cannot read the timestamp answer: {error}"))?;

    // 0 is granted, 1 granted with modifications; everything else is a
    // refusal, and a refusal quietly ignored would leave a signature that
    // looks timestamped and is not.
    if parsed.status.status > 1 {
        let reason = parsed
            .status
            .status_string
            .map(|lines| lines.join("; "))
            .unwrap_or_else(|| "no reason given".to_string());
        return Err(format!(
            "the timestamp authority refused: status {} — {reason}",
            parsed.status.status
        ));
    }

    parsed
        .token
        .ok_or_else(|| "the timestamp authority granted nothing".to_string())
}

/// Check that the authority stamped what it was asked to stamp.
///
/// A token for another imprint is not this document's timestamp, and one
/// answering another request may be an old answer replayed. Attaching either
/// would be worse than having no timestamp at all, so both are read out of the
/// token's structure rather than looked for in its bytes — a hash that happens
/// to appear inside a certificate is not a hash the authority stamped.
///
/// A token this cannot read is refused. That fails closed: a timestamp we
/// could not verify is one we should not vouch for.
fn check(token: &Any, expected_imprint: &[u8], nonce: u64) -> Result<(), String> {
    fn unreadable(what: &'static str) -> impl Fn(der::Error) -> String {
        move |error| format!("cannot read the timestamp {what}: {error}")
    }

    let content: ContentInfo = token.decode_as().map_err(unreadable("token"))?;
    let signed: SignedData = content.content.decode_as().map_err(unreadable("token"))?;
    let stamped = signed
        .encap_content_info
        .econtent
        .ok_or_else(|| "the timestamp token carries no content".to_string())?;

    // The eContent is an OCTET STRING wrapping the TSTInfo.
    let info = OctetString::from_der(&stamped.to_der().map_err(unreadable("content"))?)
        .map_err(unreadable("content"))?;
    let info = Any::from_der(info.as_bytes()).map_err(unreadable("content"))?;

    let mut reader = der::SliceReader::new(info.value()).map_err(unreadable("content"))?;
    // TSTInfo: version, policy, then the imprint (RFC 3161 §2.4.2).
    reader.tlv_bytes().map_err(unreadable("content"))?;
    reader.tlv_bytes().map_err(unreadable("content"))?;
    let imprint: MessageImprint = reader.decode().map_err(unreadable("imprint"))?;

    if imprint.hashed_message.as_bytes() != expected_imprint {
        return Err(
            "the timestamp is for a different signature than the one it was asked for".to_string(),
        );
    }

    // Then serialNumber and genTime, after which the first plain INTEGER is
    // the nonce: accuracy is a SEQUENCE, ordering a BOOLEAN, and the two
    // remaining fields are context-specific.
    reader.tlv_bytes().map_err(unreadable("content"))?;
    reader.tlv_bytes().map_err(unreadable("content"))?;
    while !reader.is_finished() {
        if reader.peek_tag().map_err(unreadable("content"))? == der::Tag::Integer {
            let echoed: u64 = reader.decode().map_err(unreadable("nonce"))?;
            if echoed != nonce {
                return Err("the timestamp does not answer this request".to_string());
            }
            return Ok(());
        }
        reader.tlv_bytes().map_err(unreadable("content"))?;
    }

    Err("the timestamp does not say which request it answers".to_string())
}

/// When the authority says it stamped, which is the one instant in a
/// signature that does not rest on the signer's own word.
pub(crate) fn stamped_at(token: &Any) -> Option<u64> {
    let content: ContentInfo = token.decode_as().ok()?;
    let signed: SignedData = content.content.decode_as().ok()?;
    let stamped = signed.encap_content_info.econtent?;
    let info = OctetString::from_der(&stamped.to_der().ok()?).ok()?;
    let info = Any::from_der(info.as_bytes()).ok()?;

    let mut reader = der::SliceReader::new(info.value()).ok()?;
    // version, policy, messageImprint, serialNumber, then genTime.
    for _ in 0..4 {
        reader.tlv_bytes().ok()?;
    }
    let time: der::asn1::GeneralizedTime = reader.decode().ok()?;
    Some(time.to_unix_duration().as_secs())
}

/// Attach a timestamp token to a signature.
///
/// It goes in as an UNSIGNED attribute, which is what lets it be added after
/// the fact: the signature covers the signed attributes, not these.
pub fn attach_timestamp(cms: &[u8], response: &[u8], nonce: u64) -> Result<Vec<u8>, String> {
    let (mut signed, signature) = signature_of(cms)?;
    let token = token_of(response)?;
    check(&token, Sha256::digest(&signature).as_slice(), nonce)?;

    let mut values = SetOfVec::new();
    values
        .insert(token)
        .map_err(|error| format!("cannot carry the timestamp: {error}"))?;
    let attribute = Attribute {
        oid: SIGNATURE_TIME_STAMP,
        values,
    };

    let mut signers = signed.signer_infos.0.into_vec();
    let Some(signer) = signers.first_mut() else {
        return Err("the signature has no signer".to_string());
    };
    match &mut signer.unsigned_attrs {
        Some(existing) => existing
            .insert(attribute)
            .map(|_| ())
            .map_err(|error| format!("cannot carry the timestamp: {error}"))?,
        None => {
            let mut attributes = SetOfVec::new();
            attributes
                .insert(attribute)
                .map_err(|error| format!("cannot carry the timestamp: {error}"))?;
            signer.unsigned_attrs = Some(attributes);
        }
    }
    signed.signer_infos = cms::signed_data::SignerInfos(
        SetOfVec::from_iter(signers)
            .map_err(|error| format!("cannot rebuild the signature: {error}"))?,
    );

    let content = ContentInfo {
        content_type: const_oid::db::rfc5911::ID_SIGNED_DATA,
        content: Any::encode_from(&signed)
            .map_err(|error| format!("cannot rebuild the signature: {error}"))?,
    };
    content
        .to_der()
        .map_err(|error| format!("cannot encode the signature: {error}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use cms::signed_data::{EncapsulatedContentInfo, SignerInfos};
    use der::asn1::GeneralizedTime;

    use super::*;
    use crate::cms::tests::key_and_certificate;
    use crate::sign_cms;

    /// `id-ct-TSTInfo`.
    const TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");

    #[derive(Sequence)]
    struct TstInfo {
        version: u32,
        policy: ObjectIdentifier,
        message_imprint: MessageImprint,
        serial_number: u32,
        gen_time: GeneralizedTime,
        nonce: u64,
    }

    /// A signature to hang a timestamp on.
    fn signature() -> Vec<u8> {
        let (key, certificate) = key_and_certificate();
        sign_cms(
            &[0x42; 32],
            &key,
            std::slice::from_ref(&certificate),
            "2026-09-04T14:30:00Z",
        )
        .expect("signing should succeed")
    }

    /// What an authority sends back, built here so the parsing can be tested
    /// without one.
    fn response(status: u32, imprint: &[u8], nonce: u64) -> Vec<u8> {
        let info = TstInfo {
            version: 1,
            policy: ObjectIdentifier::new_unwrap("1.2.3.4.1"),
            message_imprint: MessageImprint {
                hash_algorithm: sha256(),
                hashed_message: OctetString::new(imprint).unwrap(),
            },
            serial_number: 7,
            gen_time: GeneralizedTime::from_unix_duration(std::time::Duration::from_secs(
                1_788_532_200,
            ))
            .unwrap(),
            nonce,
        }
        .to_der()
        .unwrap();

        let signed = SignedData {
            version: cms::content_info::CmsVersion::V3,
            digest_algorithms: SetOfVec::new(),
            encap_content_info: EncapsulatedContentInfo {
                econtent_type: TST_INFO,
                econtent: Some(Any::encode_from(&OctetString::new(info).unwrap()).unwrap()),
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos(SetOfVec::new()),
        };
        let token = ContentInfo {
            content_type: const_oid::db::rfc5911::ID_SIGNED_DATA,
            content: Any::encode_from(&signed).unwrap(),
        };

        TimeStampResp {
            status: PkiStatusInfo {
                status,
                status_string: (status > 1).then(|| vec!["policy not supported".to_string()]),
            },
            token: (status <= 1).then(|| Any::encode_from(&token).unwrap()),
        }
        .to_der()
        .unwrap()
    }

    /// A granted answer for a given signature, so other modules can test what
    /// a timestamped signature looks like without an authority.
    pub(crate) fn granted_response(cms: &[u8], nonce: u64) -> Vec<u8> {
        response(0, &imprint_of(cms), nonce)
    }

    fn imprint_of(cms: &[u8]) -> Vec<u8> {
        let (_, value) = signature_of(cms).expect("a signature");
        Sha256::digest(&value).to_vec()
    }

    #[test]
    fn the_query_asks_for_the_signature_to_be_stamped() {
        let cms = signature();
        let (query, nonce) = timestamp_query(&cms).expect("a query");

        let parsed = TimeStampReq::from_der(&query).expect("it decodes");
        assert_eq!(parsed.version, 1);
        assert!(
            parsed.cert_req,
            "the authority has to send its certificate, or nobody can check the token"
        );
        assert_eq!(parsed.nonce, Some(nonce));
        assert_eq!(
            parsed.message_imprint.hashed_message.as_bytes(),
            imprint_of(&cms),
            "what is stamped is the signature value"
        );
    }

    /// The point of an unsigned attribute: a timestamp can be added after the
    /// fact without disturbing what was signed.
    #[test]
    fn attaching_a_timestamp_leaves_the_signature_intact() {
        let cms = signature();
        let (_, nonce) = timestamp_query(&cms).expect("a query");
        let stamped =
            attach_timestamp(&cms, &response(0, &imprint_of(&cms), nonce), nonce).expect("attach");

        let before = signature_of(&cms).expect("a signature").1;
        let after = signature_of(&stamped).expect("a signature").1;
        assert_eq!(after, before, "the signature value cannot change");

        let signed = signature_of(&stamped).expect("a signature").0;
        let signer = signed.signer_infos.0.as_slice().first().expect("a signer");
        assert!(
            signer
                .unsigned_attrs
                .as_ref()
                .expect("unsigned attributes")
                .iter()
                .any(|attribute| attribute.oid == SIGNATURE_TIME_STAMP),
            "and the timestamp is on it"
        );
    }

    /// A token for someone else's signature would say nothing about this one.
    #[test]
    fn refuses_a_timestamp_for_another_signature() {
        let cms = signature();
        let (_, nonce) = timestamp_query(&cms).expect("a query");
        let error = attach_timestamp(&cms, &response(0, &[0x99; 32], nonce), nonce)
            .expect_err("it is not ours");
        assert!(error.contains("different signature"), "got {error:?}");
    }

    /// And one answering another request may be an old answer replayed.
    #[test]
    fn refuses_a_timestamp_that_answers_another_request() {
        let cms = signature();
        let (_, nonce) = timestamp_query(&cms).expect("a query");
        let error = attach_timestamp(&cms, &response(0, &imprint_of(&cms), nonce ^ 1), nonce)
            .expect_err("it answers something else");
        assert!(error.contains("does not answer"), "got {error:?}");
    }

    /// A refusal quietly ignored would leave a signature that looks
    /// timestamped and is not.
    #[test]
    fn refuses_when_the_authority_refused() {
        let cms = signature();
        let (_, nonce) = timestamp_query(&cms).expect("a query");
        let error = attach_timestamp(&cms, &response(2, &imprint_of(&cms), nonce), nonce)
            .expect_err("the authority said no");
        assert!(error.contains("status 2"), "got {error:?}");
        assert!(
            error.contains("policy not supported"),
            "the reason it gave should reach the caller, got {error:?}"
        );
    }

    #[test]
    fn refuses_an_answer_it_cannot_read() {
        let cms = signature();
        let error =
            attach_timestamp(&cms, b"not a timestamp", 1).expect_err("that is not an answer");
        assert!(error.contains("timestamp answer"), "got {error:?}");
    }
}
