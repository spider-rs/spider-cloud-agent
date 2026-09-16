//! Checking that a release's checksum file was signed by a release key.
//!
//! `SHA256SUMS.txt` comes from the same release as the archives it lists, so
//! on its own it proves only that the archive was not damaged on the way.
//! Anyone able to publish a release could publish matching sums. The signature
//! is what ties the sums to the holder of the release key, and it is checked
//! before a single line of the sums file is read.

use minisign_verify::{PublicKey, Signature};

use super::Version;

/// The pinned release keys, one reviewable file in the repository.
const PINNED_KEYS: &str = include_str!("release-keys.pub");

/// The most a signature file may weigh. A real one is about 300 bytes.
pub const MAX_SIGNATURE_BYTES: usize = 4096;

/// The signature file's name beside `SHA256SUMS.txt`.
pub const SIGNATURE_FILE: &str = "SHA256SUMS.txt.minisig";

/// The keys a release build trusts.
pub fn pinned() -> Result<Vec<PublicKey>, String> {
    parse_keys(PINNED_KEYS)
}

/// Every key in a minisign public key file, or several concatenated. Blank
/// lines, `#` lines and `untrusted comment:` lines are skipped, and every
/// other line must be a key. A file with no key in it is refused, because an
/// empty list would trust nothing and fail every release in a confusing way.
pub fn parse_keys(text: &str) -> Result<Vec<PublicKey>, String> {
    let mut keys = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with("untrusted comment:") {
            continue;
        }
        let key = PublicKey::from_base64(line)
            .map_err(|error| format!("a release key does not decode: {error}"))?;
        keys.push(key);
    }
    if keys.is_empty() {
        return Err("no release key is pinned".to_string());
    }
    Ok(keys)
}

/// The trusted comment a release's signature must carry, word for word.
///
/// The version is in it so a validly signed checksum file from one release
/// cannot be served under another release's tag.
pub fn expected_comment(version: &Version) -> String {
    format!("spider-agent v{version} SHA256SUMS.txt")
}

/// Refuse the checksum file unless one of `keys` signed it, with the
/// prehashed algorithm `minisign -S` uses, and the signed trusted comment
/// names this version.
///
/// The comment is compared only after the signature checks out, because
/// `minisign-verify` checks the global signature over it inside `verify`, and
/// until then the comment is just text anyone could have written.
pub fn verify_sums(
    sums: &[u8],
    signature: &[u8],
    keys: &[PublicKey],
    version: &Version,
) -> Result<(), String> {
    let text =
        std::str::from_utf8(signature).map_err(|_| format!("{SIGNATURE_FILE} is not text"))?;
    let signature = Signature::decode(text)
        .map_err(|error| format!("{SIGNATURE_FILE} is malformed: {error}"))?;
    // Legacy, non-prehashed signatures are refused: every release is signed
    // by a minisign that prehashes, so one that does not was made some other
    // way.
    let signed = keys
        .iter()
        .any(|key| key.verify(sums, &signature, false).is_ok());
    if !signed {
        return Err(format!(
            "SHA256SUMS.txt is not signed by a trusted release key ({SIGNATURE_FILE} does not verify)"
        ));
    }
    let expected = expected_comment(version);
    if signature.trusted_comment() != expected {
        return Err(format!(
            "{SIGNATURE_FILE} is signed for {:?}, not {expected:?}",
            signature.trusted_comment()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn exactly_the_production_key_is_pinned() {
        let keys = pinned().unwrap();
        assert_eq!(keys.len(), 1);
        let line = PINNED_KEYS
            .lines()
            .find(|line| line.starts_with("RW"))
            .unwrap();
        assert_eq!(
            line,
            "RWQTvFK0BbbrQ5Fi5QICN6wkM5EsCGlfeLsTSiY9hTMdZpUjGaAg5t13"
        );
    }

    #[test]
    fn a_key_file_with_no_key_or_a_bad_line_is_refused() {
        assert!(parse_keys("# nothing\n\nuntrusted comment: x\n").is_err());
        assert!(parse_keys("not a key\n").is_err());
    }

    #[test]
    fn the_comment_names_the_tag_version() {
        let version = Version::parse("0.4.1").unwrap();
        assert_eq!(
            expected_comment(&version),
            "spider-agent v0.4.1 SHA256SUMS.txt"
        );
    }

    /// A throwaway key made with `minisign -G -W` for this test and nothing
    /// else, and a signature made by the minisign binary with
    /// `minisign -S -m SUMS -t "spider-agent v1.2.3 SHA256SUMS.txt"` over the
    /// bytes `x  y\n`. It pins the format real releases are signed in, where
    /// the integration tests sign with their own code.
    const TEST_ONLY_MINISIGN_KEY: &str = "RWREj0pXtEnhygvOWHHEDTbIoK6Udht/kdytjQUirAs80QC7EVvD1FJb";
    const TEST_ONLY_MINISIGN_SIGNATURE: &str =
        "untrusted comment: signature from minisign secret key
RUREj0pXtEnhyupTLfBJzdajxgAFu27cAJWkjCEMK/sqGs3pgqT5atKJzOaM6AkQNmI8AB1DwG03wB0hLGTVFWhgbCgLTIzHbgA=
trusted comment: spider-agent v1.2.3 SHA256SUMS.txt
dM1WqloiQzZN7KA3SAqnLiFgsjE5BXGuau4JVpvsOi6dEf+pZielK58mdNKrJhtntQRjJtXkQ93eY99H1oprAQ==
";

    #[test]
    fn a_signature_from_the_minisign_binary_verifies_and_is_held_to_its_version() {
        let keys = parse_keys(TEST_ONLY_MINISIGN_KEY).unwrap();
        let signature = TEST_ONLY_MINISIGN_SIGNATURE.as_bytes();
        let version = Version::parse("1.2.3").unwrap();
        verify_sums(b"x  y\n", signature, &keys, &version).unwrap();
        assert!(verify_sums(b"x  z\n", signature, &keys, &version).is_err());
        let newer = Version::parse("1.2.4").unwrap();
        assert!(verify_sums(b"x  y\n", signature, &keys, &newer).is_err());
        let production = pinned().unwrap();
        assert!(verify_sums(b"x  y\n", signature, &production, &version).is_err());
    }

    #[test]
    fn a_malformed_signature_is_refused() {
        let keys = pinned().unwrap();
        let version = Version::parse("0.4.1").unwrap();
        let refused = verify_sums(b"sums", b"not a signature", &keys, &version).unwrap_err();
        assert!(refused.contains("malformed"), "{refused}");
    }
}
