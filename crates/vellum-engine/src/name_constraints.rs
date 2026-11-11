//! The names an authority may certify, RFC 5280 §4.2.1.10.
//!
//! An authority can be delegated under constraints: a company's own
//! authority, say, allowed to certify `O=Acme` and nobody else. Unchecked, the
//! constraint is decoration, and the delegated authority can certify anyone and
//! still be trusted. Every name a certificate below it carries — its subject,
//! the e-mail addresses in that subject, its alternative names — is held to
//! the constraints of each authority above it.
//!
//! Names of the kinds an authority constrains and this cannot compare (other
//! names, EDI party names, registered identifiers) are refused rather than
//! waved through: a constraint not understood is a constraint not enforced.

use der::asn1::Ia5StringRef;
use der::Decode;
use x509_cert::ext::pkix::constraints::name::{GeneralSubtrees, NameConstraints};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::SubjectAltName;
use x509_cert::name::Name;
use x509_cert::Certificate;

use crate::trust::extension;

/// `emailAddress`, the attribute an e-mail address is often written in when a
/// subject has no alternative names.
const EMAIL_ADDRESS: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.1");

/// One name a certificate is known by, in the form constraints compare.
enum Named<'a> {
    Directory(&'a Name),
    Dns(String),
    Email(String),
    Ip(&'a [u8]),
    Uri(String),
    /// A kind this cannot compare, by its tag in `GeneralName`.
    Other(u8),
}

impl Named<'_> {
    fn kind(&self) -> u8 {
        match self {
            Self::Directory(_) => 4,
            Self::Dns(_) => 2,
            Self::Email(_) => 1,
            Self::Ip(_) => 7,
            Self::Uri(_) => 6,
            Self::Other(kind) => *kind,
        }
    }
}

fn kind_of(name: &GeneralName) -> u8 {
    match name {
        GeneralName::OtherName(_) => 0,
        GeneralName::Rfc822Name(_) => 1,
        GeneralName::DnsName(_) => 2,
        GeneralName::DirectoryName(_) => 4,
        GeneralName::EdiPartyName(_) => 5,
        GeneralName::UniformResourceIdentifier(_) => 6,
        GeneralName::IpAddress(_) => 7,
        GeneralName::RegisteredId(_) => 8,
    }
}

/// Every name `certificate` is known by.
fn names_of(certificate: &Certificate) -> Vec<Named<'_>> {
    let subject = &certificate.tbs_certificate.subject;
    let mut names = Vec::new();
    if !subject.0.is_empty() {
        names.push(Named::Directory(subject));
    }
    for attribute in subject.0.iter().flat_map(|rdn| rdn.0.iter()) {
        if attribute.oid == EMAIL_ADDRESS {
            let address = attribute
                .value
                .decode_as::<Ia5StringRef<'_>>()
                .map(|address| address.as_str().to_string())
                .unwrap_or_default();
            names.push(Named::Email(address));
        }
    }
    names
}

fn alternative_names(certificate: &Certificate) -> Vec<GeneralName> {
    extension::<SubjectAltName>(certificate)
        .map(|names| names.0)
        .unwrap_or_default()
}

fn named(name: &GeneralName) -> Named<'_> {
    match name {
        GeneralName::DirectoryName(directory) => Named::Directory(directory),
        GeneralName::DnsName(dns) => Named::Dns(dns.as_str().to_ascii_lowercase()),
        GeneralName::Rfc822Name(email) => Named::Email(email.as_str().to_string()),
        GeneralName::IpAddress(address) => Named::Ip(address.as_bytes()),
        GeneralName::UniformResourceIdentifier(uri) => Named::Uri(uri.as_str().to_string()),
        other => Named::Other(kind_of(other)),
    }
}

/// `host` is `domain` itself or below it. A constraint starting with a dot
/// only covers what is below it.
fn within_domain(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.to_ascii_lowercase();
    if domain.is_empty() {
        return true;
    }
    if let Some(below) = domain.strip_prefix('.') {
        return host.ends_with(&format!(".{below}"));
    }
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// RFC 5280 §4.2.1.10: a mailbox, every mailbox at a host, or every mailbox
/// below a domain.
fn email_within(address: &str, constraint: &str) -> bool {
    let Some((_, host)) = address.rsplit_once('@') else {
        return false;
    };
    if constraint.contains('@') {
        return address.eq_ignore_ascii_case(constraint);
    }
    if constraint.starts_with('.') {
        return within_domain(host, constraint);
    }
    host.eq_ignore_ascii_case(constraint)
}

/// The host of a URI, without user information or port.
fn uri_host(uri: &str) -> Option<&str> {
    let (_, rest) = uri.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host_and_port.starts_with('[') {
        return host_and_port.split(']').next().map(|host| &host[1..]);
    }
    host_and_port.split(':').next()
}

/// Whether `name` falls under `base`. `None` when the two cannot be compared,
/// which the caller refuses.
fn within(name: &Named<'_>, base: &GeneralName) -> Option<bool> {
    match (name, base) {
        (Named::Directory(name), GeneralName::DirectoryName(base)) => {
            Some(name.0.len() >= base.0.len() && name.0[..base.0.len()] == base.0[..])
        }
        (Named::Dns(host), GeneralName::DnsName(domain)) => {
            Some(within_domain(host, domain.as_str()))
        }
        (Named::Email(address), GeneralName::Rfc822Name(constraint)) => {
            Some(email_within(address, constraint.as_str()))
        }
        (Named::Ip(address), GeneralName::IpAddress(range)) => {
            let range = range.as_bytes();
            if range.len() != address.len() * 2 {
                return Some(false);
            }
            let (network, mask) = range.split_at(address.len());
            Some(
                address
                    .iter()
                    .zip(network)
                    .zip(mask)
                    .all(|((byte, net), mask)| byte & mask == net & mask),
            )
        }
        (Named::Uri(uri), GeneralName::UniformResourceIdentifier(domain)) => {
            Some(uri_host(uri).is_some_and(|host| within_domain(host, domain.as_str())))
        }
        _ => None,
    }
}

fn of_kind(subtrees: &GeneralSubtrees, kind: u8) -> impl Iterator<Item = &GeneralName> {
    subtrees
        .iter()
        .map(|subtree| &subtree.base)
        .filter(move |base| kind_of(base) == kind)
}

/// Whether `certificate`'s names all fall within what `authority` may
/// certify. `Err` names the first that does not.
pub(crate) fn check(authority: &Certificate, certificate: &Certificate) -> Result<(), String> {
    let Some(constraints) = authority
        .tbs_certificate
        .extensions
        .as_ref()
        .and_then(|extensions| {
            extensions.iter().find(|extension| {
                extension.extn_id == <NameConstraints as const_oid::AssociatedOid>::OID
            })
        })
    else {
        return Ok(());
    };
    let constraints =
        NameConstraints::from_der(constraints.extn_value.as_bytes()).map_err(|_| {
            format!(
                "the name constraints of {} cannot be read",
                authority.tbs_certificate.subject
            )
        })?;

    let alternatives = alternative_names(certificate);
    let mut names = names_of(certificate);
    names.extend(alternatives.iter().map(named));

    let refused = |what: &str| {
        Err(format!(
            "{} certified {} outside the names it may certify ({what})",
            authority.tbs_certificate.subject, certificate.tbs_certificate.subject
        ))
    };
    for name in &names {
        if let Some(permitted) = &constraints.permitted_subtrees {
            let mut bases = of_kind(permitted, name.kind()).peekable();
            if bases.peek().is_some() {
                let mut allowed = false;
                for base in bases {
                    match within(name, base) {
                        Some(true) => allowed = true,
                        Some(false) => {}
                        None => return refused("a name of a kind this cannot compare"),
                    }
                }
                if !allowed {
                    return refused("not among the permitted names");
                }
            }
        }
        if let Some(excluded) = &constraints.excluded_subtrees {
            for base in of_kind(excluded, name.kind()) {
                match within(name, base) {
                    Some(false) => {}
                    Some(true) => return refused("among the excluded names"),
                    None => return refused("a name of a kind this cannot compare"),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_covers_itself_and_what_is_below_it() {
        assert!(within_domain("acme.test", "acme.test"));
        assert!(within_domain("www.acme.test", "acme.test"));
        assert!(!within_domain("notacme.test", "acme.test"));
        assert!(!within_domain("acme.test", ".acme.test"));
        assert!(within_domain("www.acme.test", ".acme.test"));
    }

    #[test]
    fn a_mailbox_constraint_reads_the_host() {
        assert!(email_within("alice@acme.test", "acme.test"));
        assert!(!email_within("alice@mail.acme.test", "acme.test"));
        assert!(email_within("alice@mail.acme.test", ".acme.test"));
        assert!(email_within("alice@acme.test", "alice@acme.test"));
        assert!(!email_within("bob@acme.test", "alice@acme.test"));
    }

    #[test]
    fn a_uri_is_judged_by_its_host() {
        assert_eq!(
            uri_host("https://user@www.acme.test:8443/path"),
            Some("www.acme.test")
        );
        assert_eq!(uri_host("http://[::1]:80/"), Some("::1"));
        assert_eq!(uri_host("not a uri"), None);
    }
}
