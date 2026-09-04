//! Building the signature value itself, for a key we hold.
//!
//! [`crate::sign`] reserves the space and says what has to be signed; this
//! turns that digest into the CMS `SignedData` that goes in the space. The two
//! are separate on purpose — a certified provider returns a `SignedData` of
//! its own and never comes through here.
//!
//! What PAdES asks for beyond a plain CMS signature (ETSI EN 319 142, and
//! CAdES before it) is that the signed attributes carry a reference to the
//! signing certificate. Without it a signature is bound to a key but not to an
//! identity: anyone holding a certificate for that key could claim it. The
//! attribute is `signing-certificate-v2`, and it is why this module builds the
//! attributes rather than taking the defaults.
//!
//! The key never leaves this function, and the document never enters it.

use cms::attr::SigningTime;
use cms::builder::{create_signing_time_attribute, SignedDataBuilder, SignerInfoBuilder};
use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{EncapsulatedContentInfo, SignerIdentifier};
use const_oid::db::rfc5911::ID_DATA;
use der::asn1::{OctetString, SetOfVec, UtcTime};
use der::{Any, Decode, Encode, Sequence};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;

/// `id-aa-signingCertificateV2`, RFC 5035 §3.
const SIGNING_CERTIFICATE_V2: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
/// `id-sha256`, the digest everything here uses.
const SHA_256: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");

/// One certificate, identified by the hash of it.
///
/// The hash algorithm is `DEFAULT id-sha256`, so a SHA-256 one is left out —
/// and encoding a DEFAULT value is not merely redundant in DER, it is invalid.
#[derive(Sequence)]
struct EssCertIdV2 {
    cert_hash: OctetString,
}

#[derive(Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

fn algorithm(oid: const_oid::ObjectIdentifier) -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid,
        parameters: None,
    }
}

/// The attribute binding this signature to the certificate that made it.
fn signing_certificate(certificate: &Certificate) -> Result<Attribute, String> {
    let encoded = certificate
        .to_der()
        .map_err(|error| format!("cannot re-encode the certificate: {error}"))?;
    let hash = Sha256::digest(&encoded);

    let value = SigningCertificateV2 {
        certs: vec![EssCertIdV2 {
            cert_hash: OctetString::new(hash.as_slice())
                .map_err(|error| format!("cannot encode the certificate hash: {error}"))?,
        }],
    }
    .to_der()
    .map_err(|error| format!("cannot encode the signing certificate: {error}"))?;

    let mut values = SetOfVec::new();
    values
        .insert(
            Any::from_der(&value)
                .map_err(|error| format!("cannot encode the signing certificate: {error}"))?,
        )
        .map_err(|error| format!("cannot encode the signing certificate: {error}"))?;

    Ok(Attribute {
        oid: SIGNING_CERTIFICATE_V2,
        values,
    })
}

/// When the signature was made, as the caller states it rather than as this
/// machine's clock happens to read.
fn signing_time(iso: &str) -> Result<Attribute, String> {
    let Some(time) = parse_iso(iso) else {
        // The clock of the machine building the signature is a defensible
        // fallback; a wrong date silently written as if it were the caller's
        // is not.
        return create_signing_time_attribute()
            .map_err(|error| format!("cannot encode the signing time: {error}"));
    };

    let utc = UtcTime::from_unix_duration(std::time::Duration::from_secs(time))
        .map_err(|error| format!("cannot encode the signing time: {error}"))?;
    let mut values = SetOfVec::new();
    values
        .insert(
            Any::from_der(
                &SigningTime::UtcTime(utc)
                    .to_der()
                    .map_err(|error| format!("cannot encode the signing time: {error}"))?,
            )
            .map_err(|error| format!("cannot encode the signing time: {error}"))?,
        )
        .map_err(|error| format!("cannot encode the signing time: {error}"))?;

    Ok(Attribute {
        oid: const_oid::db::rfc5911::ID_SIGNING_TIME,
        values,
    })
}

/// Seconds since the epoch for an ISO 8601 instant in UTC.
fn parse_iso(iso: &str) -> Option<u64> {
    let digits: Vec<u32> = iso
        .chars()
        .take_while(|character| *character != '+' && *character != 'Z')
        .filter(|character| character.is_ascii_digit())
        .map(|character| character.to_digit(10).unwrap_or(0))
        .collect();
    if digits.len() < 14 {
        return None;
    }
    let number = |from: usize, len: usize| -> u64 {
        digits[from..from + len]
            .iter()
            .fold(0u64, |total, digit| total * 10 + u64::from(*digit))
    };

    let (year, month, day) = (number(0, 4), number(4, 2), number(6, 2));
    let (hour, minute, second) = (number(8, 2), number(10, 2), number(12, 2));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Days since the epoch, by the civil-calendar algorithm — no dependency on
    // a date library for one conversion.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Turn a digest into the CMS `SignedData` that goes into the document.
///
/// `certificates` is the signer's certificate first, then any chain. The key
/// is PKCS#8 DER, and the certificates are DER — not a PKCS#12 bundle, whose
/// parsing in Rust is not something to put under a signature.
pub fn sign_cms(
    digest: &[u8],
    key: &[u8],
    certificates: &[Vec<u8>],
    signed_at: &str,
) -> Result<Vec<u8>, String> {
    let Some((signer_der, chain)) = certificates.split_first() else {
        return Err("signing needs at least the signer's certificate".to_string());
    };

    let private_key = RsaPrivateKey::from_pkcs8_der(key)
        .map_err(|error| format!("cannot read the private key: {error}"))?;
    let signing_key = SigningKey::<Sha256>::new(private_key);

    let signer = Certificate::from_der(signer_der)
        .map_err(|error| format!("cannot read the signer's certificate: {error}"))?;
    let serial: SerialNumber = signer.tbs_certificate.serial_number.clone();
    let sid = SignerIdentifier::IssuerAndSerialNumber(cms::cert::IssuerAndSerialNumber {
        issuer: signer.tbs_certificate.issuer.clone(),
        serial_number: serial,
    });

    // Detached: the document is not carried in the signature, only its digest.
    let content = EncapsulatedContentInfo {
        econtent_type: ID_DATA,
        econtent: None,
    };

    let mut signer_info = SignerInfoBuilder::new(
        &signing_key,
        sid,
        algorithm(SHA_256),
        &content,
        Some(digest),
    )
    .map_err(|error| format!("cannot start the signature: {error}"))?;
    signer_info
        .add_signed_attribute(signing_time(signed_at)?)
        .map_err(|error| format!("cannot add the signing time: {error}"))?;
    signer_info
        .add_signed_attribute(signing_certificate(&signer)?)
        .map_err(|error| format!("cannot add the signing certificate: {error}"))?;

    let mut builder = SignedDataBuilder::new(&content);
    builder
        .add_digest_algorithm(algorithm(SHA_256))
        .map_err(|error| format!("cannot declare the digest algorithm: {error}"))?;
    builder
        .add_certificate(CertificateChoices::Certificate(signer))
        .map_err(|error| format!("cannot carry the signer's certificate: {error}"))?;
    for certificate in chain {
        let certificate = Certificate::from_der(certificate)
            .map_err(|error| format!("cannot read a chain certificate: {error}"))?;
        builder
            .add_certificate(CertificateChoices::Certificate(certificate))
            .map_err(|error| format!("cannot carry a chain certificate: {error}"))?;
    }
    builder
        .add_signer_info::<SigningKey<Sha256>, rsa::pkcs1v15::Signature>(signer_info)
        .map_err(|error| format!("cannot sign: {error}"))?;

    let signed: ContentInfo = builder
        .build()
        .map_err(|error| format!("cannot assemble the signature: {error}"))?;
    signed
        .to_der()
        .map_err(|error| format!("cannot encode the signature: {error}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::str::FromStr;
    use std::time::Duration;

    use der::asn1::BitString;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::signature::Verifier;
    use x509_cert::builder::{Builder, CertificateBuilder, Profile};
    use x509_cert::name::Name;
    use x509_cert::spki::SubjectPublicKeyInfoOwned;
    use x509_cert::time::Validity;

    use super::*;

    /// A throwaway key and a certificate for it, made fresh each run.
    ///
    /// Checking a private key into the repository — even a worthless one —
    /// trips every scanner that exists and teaches the wrong habit, so the
    /// tests build their own.
    pub(crate) fn key_and_certificate() -> (Vec<u8>, Vec<u8>) {
        let mut rng = rsa::rand_core::OsRng;
        // Short, because the tests only need the arithmetic to be real.
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("a key");
        let signing_key = SigningKey::<Sha256>::new(private_key.clone());

        let subject = Name::from_str("CN=Vellum Test,O=Vellum").expect("a name");
        let public = SubjectPublicKeyInfoOwned {
            algorithm: AlgorithmIdentifierOwned {
                oid: const_oid::db::rfc5912::RSA_ENCRYPTION,
                parameters: Some(Any::null()),
            },
            subject_public_key: BitString::from_der(
                &rsa::pkcs1::EncodeRsaPublicKey::to_pkcs1_der(&private_key.to_public_key())
                    .map(|document| {
                        BitString::new(0, document.as_bytes())
                            .expect("a bit string")
                            .to_der()
                            .expect("encodable")
                    })
                    .expect("a public key"),
            )
            .expect("a bit string"),
        };

        let builder = CertificateBuilder::new(
            Profile::Root,
            SerialNumber::from(1u32),
            Validity::from_now(Duration::from_secs(3600)).expect("a validity"),
            subject,
            public,
            &signing_key,
        )
        .expect("a certificate builder");
        let certificate: Certificate = builder.build().expect("a certificate");

        (
            private_key
                .to_pkcs8_der()
                .expect("the key encodes")
                .as_bytes()
                .to_vec(),
            certificate.to_der().expect("the certificate encodes"),
        )
    }

    fn signed() -> (Vec<u8>, [u8; 32], Vec<u8>) {
        let (key, certificate) = key_and_certificate();
        let digest = [0x42; 32];
        let cms = sign_cms(
            &digest,
            &key,
            std::slice::from_ref(&certificate),
            "2026-09-04T14:30:00Z",
        )
        .expect("signing should succeed");
        (cms, digest, certificate)
    }

    /// The signature has to verify — and it is verified here through the
    /// verifying half of the crate, not by re-running the code that made it.
    #[test]
    fn the_signature_verifies_against_the_certificate() {
        let (cms, _, certificate) = signed();

        let content = ContentInfo::from_der(&cms).expect("a content info");
        let data = content
            .content
            .decode_as::<cms::signed_data::SignedData>()
            .expect("signed data");
        let signer = data.signer_infos.0.as_slice().first().expect("a signer");

        // What was signed is the DER of the signed attributes, tagged as a SET
        // rather than with the implicit [0] they carry inside the structure.
        let attributes = signer
            .signed_attrs
            .as_ref()
            .expect("signed attributes")
            .to_der()
            .expect("encodable");

        let certificate = Certificate::from_der(&certificate).expect("the certificate");
        let spki = certificate
            .tbs_certificate
            .subject_public_key_info
            .to_der()
            .expect("the public key encodes");
        let public_key = rsa::RsaPublicKey::try_from(
            rsa::pkcs8::SubjectPublicKeyInfoRef::from_der(&spki).expect("a public key"),
        )
        .expect("an RSA key");

        rsa::pkcs1v15::VerifyingKey::<Sha256>::new(public_key)
            .verify(
                &attributes,
                &rsa::pkcs1v15::Signature::try_from(signer.signature.as_bytes())
                    .expect("a signature"),
            )
            .expect("the signature must verify");
    }

    /// The digest of the document has to be what the signature commits to.
    /// A signature over the right structure but the wrong digest would verify
    /// and mean nothing.
    #[test]
    fn the_message_digest_is_the_documents_own() {
        let (cms, digest, _) = signed();

        let content = ContentInfo::from_der(&cms).expect("a content info");
        let data = content
            .content
            .decode_as::<cms::signed_data::SignedData>()
            .expect("signed data");
        let signer = data.signer_infos.0.as_slice().first().expect("a signer");
        let attributes = signer.signed_attrs.as_ref().expect("signed attributes");

        let carried = attributes
            .iter()
            .find(|attribute| attribute.oid == const_oid::db::rfc5911::ID_MESSAGE_DIGEST)
            .and_then(|attribute| attribute.values.as_slice().first())
            .map(|value| value.value().to_vec())
            .expect("a message digest");

        assert_eq!(
            OctetString::from_der(&[&[0x04, carried.len() as u8][..], &carried[..]].concat())
                .map(|octets| octets.as_bytes().to_vec())
                .unwrap_or(carried),
            digest.to_vec(),
            "the signature commits to the document's digest"
        );
    }

    /// PAdES requires the signature to name the certificate that made it.
    /// Without it a signature is bound to a key but not to an identity.
    #[test]
    fn the_signing_certificate_is_named() {
        let (cms, _, certificate) = signed();

        let content = ContentInfo::from_der(&cms).expect("a content info");
        let data = content
            .content
            .decode_as::<cms::signed_data::SignedData>()
            .expect("signed data");
        let signer = data.signer_infos.0.as_slice().first().expect("a signer");
        let attribute = signer
            .signed_attrs
            .as_ref()
            .expect("signed attributes")
            .iter()
            .find(|attribute| attribute.oid == SIGNING_CERTIFICATE_V2)
            .expect("the signing certificate attribute");

        let value = attribute.values.as_slice().first().expect("a value");
        let hash = Sha256::digest(&certificate);
        assert!(
            value
                .value()
                .windows(hash.len())
                .any(|window| window == hash.as_slice()),
            "it has to hold the hash of the certificate that signed"
        );
    }

    #[test]
    fn the_certificate_travels_with_the_signature() {
        let (cms, _, _) = signed();
        let content = ContentInfo::from_der(&cms).expect("a content info");
        let data = content
            .content
            .decode_as::<cms::signed_data::SignedData>()
            .expect("signed data");

        assert_eq!(
            data.certificates.map(|set| set.0.len()),
            Some(1),
            "a verifier needs the certificate, and cannot be assumed to have it"
        );
    }

    #[test]
    fn refuses_to_sign_without_a_certificate() {
        let (key, _) = key_and_certificate();
        let error = sign_cms(&[0x42; 32], &key, &[], "2026-09-04T14:30:00Z")
            .expect_err("there is nothing to sign as");
        assert!(error.contains("certificate"), "got {error:?}");
    }

    #[test]
    fn refuses_a_key_it_cannot_read() {
        let error = sign_cms(
            &[0x42; 32],
            b"not a key",
            &[vec![0]],
            "2026-09-04T14:30:00Z",
        )
        .expect_err("that is not a key");
        assert!(error.contains("private key"), "got {error:?}");
    }

    /// Write the fixture the TypeScript tests use.
    ///
    /// They need a real CMS to hang a timestamp on, and cannot build one:
    /// minting a certificate is not something Node does. What it writes holds
    /// a certificate and a signature and no private key, so it is safe to keep
    /// in the repository.
    ///
    ///     cargo test -p vellum-engine write_the_typescript_fixture -- --ignored
    #[test]
    #[ignore = "regenerates a checked-in fixture"]
    fn write_the_typescript_fixture() {
        let (key, certificate) = key_and_certificate();
        let cms = sign_cms(
            &[0x42; 32],
            &key,
            std::slice::from_ref(&certificate),
            "2026-09-04T14:30:00Z",
        )
        .expect("signing should succeed");
        std::fs::write("../../tests/fixtures/signature.der", cms).expect("the fixture is written");
    }

    #[test]
    fn reads_the_instant_the_caller_states() {
        // 2026-09-04T14:30:00Z
        assert_eq!(parse_iso("2026-09-04T14:30:00Z"), Some(1_788_532_200));
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("not a date"), None);
    }
}
