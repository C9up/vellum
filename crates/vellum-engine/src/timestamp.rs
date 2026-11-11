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
use der::{Any, Decode, Encode, Reader, Sequence, Tagged};
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::spki::AlgorithmIdentifierOwned;

use crate::trust::Hash;

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

/// `id-ct-TSTInfo`, the content a timestamp token signs.
const TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
/// `id-kp-timeStamping`, which a timestamp authority's certificate has to
/// carry (RFC 3161 §2.3).
const ID_KP_TIME_STAMPING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.8");

/// A `GeneralizedTime` as RFC 3161 §2.4.2 allows it in `genTime`: seconds,
/// optionally a fraction (dropped), and `Z`. `der` refuses the fraction, which
/// real authorities write.
fn gen_time(bytes: &[u8]) -> Option<u64> {
    let (last, body) = bytes.split_last()?;
    if *last != b'Z' || body.len() < 14 {
        return None;
    }
    let (seconds, fraction) = body.split_at(14);
    let fraction_ok = match fraction.split_first() {
        None => true,
        Some((b'.', digits)) => !digits.is_empty() && digits.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    };
    if !fraction_ok || !seconds.iter().all(u8::is_ascii_digit) {
        return None;
    }
    crate::cms::parse_iso(std::str::from_utf8(seconds).ok()?)
}

/// A signed attribute's single value, as DER.
fn attribute_value(
    attributes: &x509_cert::attr::Attributes,
    oid: ObjectIdentifier,
) -> Option<Vec<u8>> {
    let mut matching = attributes.iter().filter(|attribute| attribute.oid == oid);
    let attribute = matching.next()?;
    if matching.next().is_some() || attribute.values.len() != 1 {
        return None;
    }
    attribute.values.as_slice().first()?.to_der().ok()
}

/// Check a timestamp token over the signature value `signature`, and return
/// the instant its authority vouches for.
///
/// The token sits in an unsigned attribute: anybody holding the document can
/// add or replace one, so nothing in it is read until all of this holds. The
/// authority's CMS signature over the TSTInfo verifies with the certificate the
/// token names; that certificate is for timestamping and chains to one of
/// `anchors` at `now`, and was valid when it stamped; and the TSTInfo stamps
/// this very signature value. A token failing any of it is not a timestamp.
pub(crate) fn verify_token(
    token: &Any,
    signature: &[u8],
    anchors: &[x509_cert::Certificate],
    now: u64,
) -> Result<u64, String> {
    fn unreadable(error: der::Error) -> String {
        format!("the token is not readable: {error}")
    }

    let content: ContentInfo = token.decode_as().map_err(unreadable)?;
    if content.content_type != const_oid::db::rfc5911::ID_SIGNED_DATA {
        return Err("the token is not a signed structure".to_string());
    }
    let signed: SignedData = content.content.decode_as().map_err(unreadable)?;
    if signed.encap_content_info.econtent_type != TST_INFO {
        return Err("the token does not carry a timestamp".to_string());
    }
    let stamped = signed
        .encap_content_info
        .econtent
        .as_ref()
        .ok_or_else(|| "the token carries no content".to_string())?;
    let info = OctetString::from_der(&stamped.to_der().map_err(unreadable)?).map_err(unreadable)?;

    // TSTInfo: version, policy, messageImprint, serialNumber, genTime.
    let outer = Any::from_der(info.as_bytes()).map_err(unreadable)?;
    if outer.tag() != der::Tag::Sequence {
        return Err("the token's content is not a TSTInfo".to_string());
    }
    let mut reader = der::SliceReader::new(outer.value()).map_err(unreadable)?;
    reader.tlv_bytes().map_err(unreadable)?;
    reader.tlv_bytes().map_err(unreadable)?;
    let imprint: MessageImprint = reader.decode().map_err(unreadable)?;
    reader.tlv_bytes().map_err(unreadable)?;
    let time: Any = reader.decode().map_err(unreadable)?;
    if time.tag() != der::Tag::GeneralizedTime {
        return Err("the token states no time".to_string());
    }
    let at =
        gen_time(time.value()).ok_or_else(|| "the token's time is not readable".to_string())?;

    let imprint_hash = Hash::of_digest(&imprint.hash_algorithm.oid)
        .ok_or_else(|| "the token stamps with a digest this cannot check".to_string())?;
    if imprint.hashed_message.as_bytes() != imprint_hash.digest(signature).as_slice() {
        return Err("the token stamps another signature than this one".to_string());
    }

    let [signer] = signed.signer_infos.0.as_slice() else {
        return Err("the token does not have exactly one signer".to_string());
    };
    let attributes = signer
        .signed_attrs
        .as_ref()
        .ok_or_else(|| "the token's signer carries no signed attributes".to_string())?;
    let digest = Hash::of_digest(&signer.digest_alg.oid)
        .ok_or_else(|| "the token is signed over a digest this cannot check".to_string())?;
    let content_type = attribute_value(attributes, const_oid::db::rfc5911::ID_CONTENT_TYPE)
        .and_then(|der| ObjectIdentifier::from_der(&der).ok());
    if content_type != Some(TST_INFO) {
        return Err("the token's signer does not sign a timestamp".to_string());
    }
    let committed = attribute_value(attributes, const_oid::db::rfc5911::ID_MESSAGE_DIGEST)
        .and_then(|der| OctetString::from_der(&der).ok());
    if committed.as_ref().map(OctetString::as_bytes)
        != Some(digest.digest(info.as_bytes()).as_slice())
    {
        return Err("the token's signature does not cover its content".to_string());
    }

    let certificates = crate::trust::carried(signed.certificates.as_ref());
    let authority = crate::trust::named_certificate(&signer.sid, &certificates)
        .ok_or_else(|| "the token carries no certificate for its signer".to_string())?;
    let hash = Hash::of_signature(
        &signer.signature_algorithm.oid,
        Some(&signer.digest_alg.oid),
    )
    .ok_or_else(|| "the token is signed with an algorithm this cannot check".to_string())?;
    let signed_bytes = attributes.to_der().map_err(unreadable)?;
    if !crate::trust::rsa_verifies(
        &authority.tbs_certificate.subject_public_key_info,
        hash,
        &signed_bytes,
        signer.signature.as_bytes(),
    )? {
        return Err("the token is not signed by the authority it names".to_string());
    }

    if !crate::trust::has_extended_usage(authority, ID_KP_TIME_STAMPING) {
        return Err("the token's signer is not a timestamp authority".to_string());
    }
    if !crate::trust::valid_at(authority, at) {
        return Err("the authority's certificate was not valid when it stamped".to_string());
    }
    let trust = crate::trust::evaluate(authority, &certificates, anchors, now);
    if !trust.trusted {
        return Err(format!(
            "the authority is not trusted: {}",
            trust.problems.join("; ")
        ));
    }

    // A stamp dated after now proves nothing about now.
    Ok(at.min(now))
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

    /// 2026-09-04T14:30:00Z, when the test authority stamps.
    pub(crate) const STAMPED_AT: u64 = 1_788_532_200;

    /// A token stamping `imprint`, signed by the authority whose key and
    /// certificate are given, or by nobody.
    pub(crate) fn token(
        imprint: &[u8],
        nonce: u64,
        gen_time: u64,
        authority: Option<(&[u8], &[u8])>,
    ) -> Any {
        let info = TstInfo {
            version: 1,
            policy: ObjectIdentifier::new_unwrap("1.2.3.4.1"),
            message_imprint: MessageImprint {
                hash_algorithm: sha256(),
                hashed_message: OctetString::new(imprint).unwrap(),
            },
            serial_number: 7,
            gen_time: GeneralizedTime::from_unix_duration(std::time::Duration::from_secs(gen_time))
                .unwrap(),
            nonce,
        }
        .to_der()
        .unwrap();
        let content = EncapsulatedContentInfo {
            econtent_type: TST_INFO,
            econtent: Some(Any::encode_from(&OctetString::new(info).unwrap()).unwrap()),
        };

        let Some((key, certificate)) = authority else {
            let signed = SignedData {
                version: cms::content_info::CmsVersion::V3,
                digest_algorithms: SetOfVec::new(),
                encap_content_info: content,
                certificates: None,
                crls: None,
                signer_infos: SignerInfos(SetOfVec::new()),
            };
            return Any::encode_from(&ContentInfo {
                content_type: const_oid::db::rfc5911::ID_SIGNED_DATA,
                content: Any::encode_from(&signed).unwrap(),
            })
            .unwrap();
        };

        let signing = rsa::pkcs1v15::SigningKey::<Sha256>::new(
            <rsa::RsaPrivateKey as rsa::pkcs8::DecodePrivateKey>::from_pkcs8_der(key).unwrap(),
        );
        let certificate = x509_cert::Certificate::from_der(certificate).unwrap();
        let sid = cms::signed_data::SignerIdentifier::IssuerAndSerialNumber(
            cms::cert::IssuerAndSerialNumber {
                issuer: certificate.tbs_certificate.issuer.clone(),
                serial_number: certificate.tbs_certificate.serial_number.clone(),
            },
        );
        let signer =
            cms::builder::SignerInfoBuilder::new(&signing, sid, sha256(), &content, None).unwrap();
        let mut builder = cms::builder::SignedDataBuilder::new(&content);
        builder.add_digest_algorithm(sha256()).unwrap();
        builder
            .add_certificate(cms::cert::CertificateChoices::Certificate(certificate))
            .unwrap();
        builder
            .add_signer_info::<rsa::pkcs1v15::SigningKey<Sha256>, rsa::pkcs1v15::Signature>(signer)
            .unwrap();
        Any::encode_from(&builder.build().unwrap()).unwrap()
    }

    /// What an authority sends back, built here so the parsing can be tested
    /// without one; signed by the test timestamp authority.
    fn response(status: u32, imprint: &[u8], nonce: u64) -> Vec<u8> {
        let (key, certificate) = crate::cms::tests::timestamp_authority();
        let token = token(imprint, nonce, STAMPED_AT, Some((&key, &certificate)));
        respond(status, token)
    }

    pub(crate) fn respond(status: u32, token: Any) -> Vec<u8> {
        TimeStampResp {
            status: PkiStatusInfo {
                status,
                status_string: (status > 1).then(|| vec!["policy not supported".to_string()]),
            },
            token: (status <= 1).then_some(token),
        }
        .to_der()
        .unwrap()
    }

    pub(crate) fn imprint_of(cms: &[u8]) -> Vec<u8> {
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
    fn anchors() -> Vec<x509_cert::Certificate> {
        let (_, _, authority) = crate::cms::tests::key_and_chain();
        vec![x509_cert::Certificate::from_der(&authority).unwrap()]
    }

    fn signature_value(cms: &[u8]) -> Vec<u8> {
        signature_of(cms).expect("a signature").1
    }

    #[test]
    fn a_token_from_a_trusted_authority_gives_its_instant() {
        let cms = signature();
        let (key, certificate) = crate::cms::tests::timestamp_authority();
        let token = token(&imprint_of(&cms), 1, STAMPED_AT, Some((&key, &certificate)));
        assert_eq!(
            verify_token(
                &token,
                &signature_value(&cms),
                &anchors(),
                crate::trust::now()
            ),
            Ok(STAMPED_AT)
        );
    }

    /// The token is an unsigned attribute: anybody holding the document can
    /// write one, with any instant in it. Unsigned, it is nothing.
    #[test]
    fn an_unsigned_token_is_not_believed() {
        let cms = signature();
        let forged = token(&imprint_of(&cms), 1, STAMPED_AT, None);
        let error = verify_token(
            &forged,
            &signature_value(&cms),
            &anchors(),
            crate::trust::now(),
        )
        .expect_err("nobody signed it");
        assert!(error.contains("exactly one signer"), "got {error:?}");
    }

    /// Signed, but by an authority of the forger's own making.
    #[test]
    fn a_token_from_an_authority_nobody_trusts_is_not_believed() {
        let cms = signature();
        let key = crate::cms::tests::fresh_key();
        let pkcs8 = crate::cms::tests::pkcs8(&key);
        let own = crate::cms::tests::issued_by(
            &pkcs8,
            "CN=Vellum Test Timestamps,O=Vellum",
            "CN=Vellum Test Timestamps,O=Vellum",
            5,
            crate::cms::tests::spki_of(&pkcs8),
            crate::cms::tests::window(1_577_836_800, 2_051_222_400),
            None,
            |builder| {
                builder
                    .add_extension(&x509_cert::ext::pkix::ExtendedKeyUsage(vec![
                        ID_KP_TIME_STAMPING,
                    ]))
                    .unwrap();
            },
        );
        let forged = token(&imprint_of(&cms), 1, STAMPED_AT, Some((&pkcs8, &own)));
        let error = verify_token(
            &forged,
            &signature_value(&cms),
            &anchors(),
            crate::trust::now(),
        )
        .expect_err("its authority is its own");
        assert!(error.contains("not trusted"), "got {error:?}");
    }

    /// A certificate that may sign documents may not stamp time.
    #[test]
    fn a_token_signed_by_a_certificate_not_for_timestamping_is_not_believed() {
        let cms = signature();
        let (key, certificate) = key_and_certificate();
        let forged = token(&imprint_of(&cms), 1, STAMPED_AT, Some((&key, &certificate)));
        let error = verify_token(
            &forged,
            &signature_value(&cms),
            &anchors(),
            crate::trust::now(),
        )
        .expect_err("a signer is not a timestamp authority");
        assert!(error.contains("not a timestamp authority"), "got {error:?}");
    }

    /// A genuine token for another signature, lifted onto this one.
    #[test]
    fn a_token_for_another_signature_is_not_believed() {
        let cms = signature();
        let (key, certificate) = crate::cms::tests::timestamp_authority();
        let lifted = token(&[0x99; 32], 1, STAMPED_AT, Some((&key, &certificate)));
        let error = verify_token(
            &lifted,
            &signature_value(&cms),
            &anchors(),
            crate::trust::now(),
        )
        .expect_err("it stamps something else");
        assert!(error.contains("another signature"), "got {error:?}");
    }

    /// Nor is one trusted when nothing is.
    #[test]
    fn a_token_is_not_believed_without_anchors() {
        let cms = signature();
        let (key, certificate) = crate::cms::tests::timestamp_authority();
        let token = token(&imprint_of(&cms), 1, STAMPED_AT, Some((&key, &certificate)));
        assert!(verify_token(&token, &signature_value(&cms), &[], crate::trust::now()).is_err());
    }

    /// A stamp dated after now proves nothing about now.
    #[test]
    fn a_token_dated_in_the_future_gives_no_later_instant_than_now() {
        let cms = signature();
        let (key, certificate) = crate::cms::tests::timestamp_authority();
        let token = token(&imprint_of(&cms), 1, STAMPED_AT, Some((&key, &certificate)));
        assert_eq!(
            verify_token(&token, &signature_value(&cms), &anchors(), STAMPED_AT + 1),
            Ok(STAMPED_AT)
        );
        let earlier = STAMPED_AT - 1;
        assert_eq!(
            verify_token(&token, &signature_value(&cms), &anchors(), earlier),
            Ok(earlier)
        );
    }

    /// Authorities write a fraction of a second, which is allowed.
    #[test]
    fn reads_a_generalized_time_with_a_fraction() {
        assert_eq!(gen_time(b"20260904143000Z"), Some(STAMPED_AT));
        assert_eq!(gen_time(b"20260904143000.125Z"), Some(STAMPED_AT));
        assert_eq!(gen_time(b"20260904143000."), None);
        assert_eq!(gen_time(b"20260904143000+0100"), None);
    }
}
