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
//! The instant matters, and it is not the signer's to choose. The path is
//! judged at the current time, unless a timestamp token whose authority's
//! signature checks out, and which chains to an anchor, proves the signature
//! existed earlier: then it is judged at the authority's instant, and a
//! certificate that expired or was revoked since does not retroactively unsign
//! anything. The `signingTime` a signer writes is signed but is still its own
//! word: a signer whose certificate has lapsed could otherwise write a date
//! inside the old window and be believed (ETSI EN 319 102-1 §5.6.2.1 starts
//! the best signature time at the current time for the same reason).

use cms::signed_data::SignerIdentifier;
use const_oid::ObjectIdentifier;
use der::{Decode, Encode};
use sha2::{Digest, Sha256, Sha384, Sha512};
use x509_cert::ext::pkix::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, KeyUsages, SubjectKeyIdentifier,
};
use x509_cert::spki::SubjectPublicKeyInfoOwned;
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

/// What the signature offers about when it was made, and so which instant
/// its path is judged at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// A timestamp whose authority's signature checks out and chains to an
    /// anchor: the path is judged at the authority's instant.
    Timestamp,
    /// Only the signer's own `signingTime`, which is signed but not
    /// independent: the path is judged at the current time.
    Claimed,
    /// Nothing says when: the path is judged at the current time.
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
    /// The certificate that issued the signer's, which is who has to be asked
    /// about revocation.
    pub issuer: Option<Vec<u8>>,
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

/// The digests a signature in a certificate, a CMS or an OCSP answer may be
/// computed with. Anything else is reported as uncheckable, never assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    /// The digest an `AlgorithmIdentifier` for a digest names.
    pub(crate) fn of_digest(oid: &ObjectIdentifier) -> Option<Self> {
        match oid.to_string().as_str() {
            "2.16.840.1.101.3.4.2.1" => Some(Self::Sha256),
            "2.16.840.1.101.3.4.2.2" => Some(Self::Sha384),
            "2.16.840.1.101.3.4.2.3" => Some(Self::Sha512),
            _ => None,
        }
    }

    /// The digest an RSA signature algorithm is computed with. A CMS signer
    /// may name plain `rsaEncryption` and its digest separately, so that one
    /// takes the digest from `digest`.
    pub(crate) fn of_signature(
        signature: &ObjectIdentifier,
        digest: Option<&ObjectIdentifier>,
    ) -> Option<Self> {
        match signature.to_string().as_str() {
            "1.2.840.113549.1.1.11" => Some(Self::Sha256),
            "1.2.840.113549.1.1.12" => Some(Self::Sha384),
            "1.2.840.113549.1.1.13" => Some(Self::Sha512),
            "1.2.840.113549.1.1.1" => digest.and_then(Self::of_digest),
            _ => None,
        }
    }

    pub(crate) fn digest(self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha256 => Sha256::digest(bytes).to_vec(),
            Self::Sha384 => Sha384::digest(bytes).to_vec(),
            Self::Sha512 => Sha512::digest(bytes).to_vec(),
        }
    }
}

/// The smallest RSA modulus a signature is believed from. Below it a key can
/// be factored by somebody determined, and whoever did can sign as its
/// holder (NIST SP 800-131A has disallowed less since 2013).
pub(crate) const MIN_RSA_BITS: usize = 2048;

/// Whether an RSA PKCS#1 v1.5 `signature` over `message` was made by the key
/// in `key`.
///
/// `Err` when the key cannot be checked with at all, or is too weak to be
/// believed; `Ok(false)` when it can and the signature is not its.
pub(crate) fn rsa_verifies(
    key: &SubjectPublicKeyInfoOwned,
    hash: Hash,
    message: &[u8],
    signature: &[u8],
) -> Result<bool, String> {
    let spki = key
        .to_der()
        .map_err(|error| format!("cannot read a key: {error}"))?;
    let key = rsa::pkcs8::SubjectPublicKeyInfoRef::from_der(&spki)
        .ok()
        .and_then(|info| rsa::RsaPublicKey::try_from(info).ok())
        .ok_or_else(|| "a key is not RSA, which this cannot check with".to_string())?;
    let bits = rsa::traits::PublicKeyParts::n(&key).bits();
    if bits < MIN_RSA_BITS {
        return Err(format!(
            "a key of {bits} bits is too weak to be believed: at least {MIN_RSA_BITS} are required"
        ));
    }
    let Ok(signature) = rsa::pkcs1v15::Signature::try_from(signature) else {
        return Ok(false);
    };
    let verified = match hash {
        Hash::Sha256 => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key),
            message,
            &signature,
        ),
        Hash::Sha384 => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha384>::new(key),
            message,
            &signature,
        ),
        Hash::Sha512 => rsa::signature::Verifier::verify(
            &rsa::pkcs1v15::VerifyingKey::<Sha512>::new(key),
            message,
            &signature,
        ),
    };
    Ok(verified.is_ok())
}

/// Verify that `child` was signed by `issuer`.
pub(crate) fn signed_by(child: &Certificate, issuer: &Certificate) -> Result<(), String> {
    let tbs = child
        .tbs_certificate
        .to_der()
        .map_err(|error| format!("cannot re-encode a certificate: {error}"))?;
    let signature = child
        .signature
        .as_bytes()
        .ok_or_else(|| "a certificate's signature is malformed".to_string())?;
    let algorithm = child.signature_algorithm.oid;
    let Some(hash) = Hash::of_signature(&algorithm, None) else {
        return Err(format!(
            "a certificate is signed with {algorithm}, which this cannot check"
        ));
    };
    match rsa_verifies(
        &issuer.tbs_certificate.subject_public_key_info,
        hash,
        &tbs,
        signature,
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err("a certificate is not signed by the issuer it names".to_string()),
        Err(problem) => Err(format!("an issuer's key cannot be checked: {problem}")),
    }
}

/// Seconds since the epoch for a certificate's validity bounds.
pub(crate) fn window(certificate: &Certificate) -> (u64, u64) {
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

/// Whether a certificate was within its validity window at `at`.
pub(crate) fn valid_at(certificate: &Certificate, at: u64) -> bool {
    let (from, until) = window(certificate);
    from <= at && at <= until
}

pub(crate) fn extension<'a, T: der::Decode<'a> + const_oid::AssociatedOid>(
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

/// Whether a certificate holds the purpose `usage` among its extended key
/// usages.
pub(crate) fn has_extended_usage(certificate: &Certificate, usage: ObjectIdentifier) -> bool {
    extension::<ExtendedKeyUsage>(certificate).is_some_and(|usages| usages.0.contains(&usage))
}

/// The certificate a CMS signer names, among those it carries.
///
/// A signer identifies its certificate by issuer and serial number, or by
/// the key identifier; taking the first certificate of the set instead lets
/// the document decide which certificate the report is about.
pub(crate) fn named_certificate<'a>(
    signer: &SignerIdentifier,
    certificates: &'a [Certificate],
) -> Option<&'a Certificate> {
    certificates.iter().find(|certificate| match signer {
        SignerIdentifier::IssuerAndSerialNumber(named) => {
            certificate.tbs_certificate.issuer == named.issuer
                && certificate.tbs_certificate.serial_number == named.serial_number
        }
        SignerIdentifier::SubjectKeyIdentifier(named) => {
            extension::<SubjectKeyIdentifier>(certificate).is_some_and(|own| own == *named)
        }
    })
}

/// The plain certificates of a CMS `certificates` set.
pub(crate) fn carried(set: Option<&cms::signed_data::CertificateSet>) -> Vec<Certificate> {
    set.map(|set| {
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
    .unwrap_or_default()
}

/// The current time, in seconds since the epoch.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Whether a certificate was issued under its own name, as a renewed
/// authority's is.
fn self_issued(certificate: &Certificate) -> bool {
    certificate.tbs_certificate.subject == certificate.tbs_certificate.issuer
}

/// Whether `certificate` is the anchor `anchor`.
///
/// A trust anchor is a name and a key (RFC 5280 §6.1.1): the same
/// certificate, or one carrying the anchor's subject AND its key. A name and
/// a serial number are public, and a certificate cloning them with another key
/// is anyone's.
fn is_anchor(certificate: &Certificate, anchor: &Certificate) -> bool {
    certificate == anchor
        || (certificate.tbs_certificate.subject == anchor.tbs_certificate.subject
            && certificate.tbs_certificate.subject_public_key_info
                == anchor.tbs_certificate.subject_public_key_info)
}

/// Walk from `signer` to an anchor, checking every link.
///
/// `at` is the instant every certificate on the path has to be valid at. It
/// is the current time unless a verified timestamp moved it earlier; the
/// signer's own claim of when it signed never sets it.
pub fn evaluate(
    signer: &Certificate,
    carried: &[Certificate],
    anchors: &[Certificate],
    at: u64,
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
    let mut visited = vec![signer.clone()];
    for depth in 0..MAX_DEPTH {
        if !valid_at(&current, at) {
            report.problems.push(format!(
                "{} was not valid at the instant the signature is judged at",
                current.tbs_certificate.subject
            ));
        }

        // An anchor ends the walk: it is trusted because the caller said so,
        // not because something above it vouches for it.
        if anchors.iter().any(|anchor| is_anchor(&current, anchor)) {
            report.trusted = report.problems.is_empty();
            return report;
        }

        // Every certificate here bearing the issuer's name is a candidate,
        // the anchors first; the issuer is the one whose key signed. A decoy
        // carrying the same name is passed over, not believed.
        let mut refusal = None;
        let issuer = anchors
            .iter()
            .chain(carried.iter())
            .filter(|candidate| candidate.tbs_certificate.subject == current.tbs_certificate.issuer)
            .find(|candidate| match signed_by(&current, candidate) {
                Ok(()) => true,
                Err(problem) => {
                    refusal.get_or_insert(problem);
                    false
                }
            })
            .cloned();
        let Some(issuer) = issuer else {
            report.problems.push(refusal.unwrap_or_else(|| {
                format!(
                    "no path to a trusted anchor: nothing here issued {}",
                    current.tbs_certificate.issuer
                )
            }));
            return report;
        };
        // A certificate that signed itself, or one already on the path, ends
        // the walk short of any anchor.
        if issuer == current {
            report.problems.push(format!(
                "no path to a trusted anchor: {} signed itself and is not one",
                current.tbs_certificate.subject
            ));
            return report;
        }
        if visited.contains(&issuer) {
            report
                .problems
                .push("no path to a trusted anchor: the certificate path loops".to_string());
            return report;
        }

        // Anything that issues certificates has to say that it does, and
        // how many authorities it allows below it (RFC 5280 §4.2.1.9).
        let constraints = extension::<BasicConstraints>(&issuer);
        let issues = depth > 0 || issuer.tbs_certificate.subject != current.tbs_certificate.subject;
        if issues && !constraints.as_ref().is_some_and(|basic| basic.ca) {
            report.problems.push(format!(
                "{} signed a certificate without being an authority",
                issuer.tbs_certificate.subject
            ));
        }
        // The authorities below this one, a self-issued one (a key rollover)
        // not counting.
        let below = visited
            .iter()
            .skip(1)
            .filter(|certificate| !self_issued(certificate));
        if let Some(limit) = constraints.and_then(|basic| basic.path_len_constraint) {
            if below.clone().count() > usize::from(limit) {
                report.problems.push(format!(
                    "{} allows {limit} authorities below it, and the path has more",
                    issuer.tbs_certificate.subject
                ));
            }
        }
        // And every name it certified, directly or through those below it,
        // has to be one it may certify (RFC 5280 §4.2.1.10).
        for certified in visited.iter().take(1).chain(below) {
            if let Err(problem) = crate::name_constraints::check(&issuer, certified) {
                report.problems.push(problem);
            }
        }
        visited.push(issuer.clone());

        if depth == 0 {
            report.issuer = issuer.to_der().ok();
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
