//! Checking a release asset against its checksum file, and taking the binary
//! out of it.
//!
//! Nothing here touches the disk or the network. The order the caller follows
//! is the point: the archive bytes are hashed and compared with the line
//! `SHA256SUMS.txt` holds for them before a single byte of the archive is
//! decompressed, so an archive that did not match is never even opened.

use std::io::Read;

use sha2::{Digest, Sha256};

/// The name of the one entry a release archive must hold, at its root.
pub const BINARY_ENTRY: &str = "spider-agent";

/// The most an archive may weigh when it arrives. The release binary is under
/// three megabytes compressed, so this is room for growth and nothing more.
pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

/// The most the archive may hold once decompressed, so a small archive that
/// inflates without end cannot fill memory.
pub const MAX_UNPACKED_BYTES: usize = 128 * 1024 * 1024;

/// The most a checksum file may weigh. Four lines of a hundred bytes each.
pub const MAX_SUMS_BYTES: usize = 64 * 1024;

const BLOCK: usize = 512;

/// A SHA-256 digest.
pub type Digest32 = [u8; 32];

/// The SHA-256 of these bytes.
pub fn sha256(bytes: &[u8]) -> Digest32 {
    Sha256::digest(bytes).into()
}

/// A digest as lowercase hex, the way `shasum -a 256` prints one.
pub fn hex(digest: &Digest32) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Lowercase or uppercase hex back into a digest.
pub fn parse_hex(text: &str) -> Option<Digest32> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        let high = nibble(*pair.first()?)?;
        let low = nibble(*pair.get(1)?)?;
        *slot = (high << 4) | low;
    }
    Some(out)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The digest `SHA256SUMS.txt` lists for `file`.
///
/// Lines are `<hex>  <name>` as `shasum -a 256` writes them, or `<hex> *<name>`
/// in binary mode. A file with no line for the name is refused, and so is one
/// with two lines for it that disagree, because there is then no saying which
/// one the release meant.
pub fn expected_digest(sums: &[u8], file: &str) -> Result<Digest32, String> {
    let text = std::str::from_utf8(sums).map_err(|_| "SHA256SUMS.txt is not text".to_string())?;
    let mut found: Option<Digest32> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((digest, rest)) = line.split_once(' ') else {
            continue;
        };
        let name = rest
            .strip_prefix(' ')
            .or_else(|| rest.strip_prefix('*'))
            .unwrap_or(rest);
        if name != file {
            continue;
        }
        let digest =
            parse_hex(digest).ok_or_else(|| format!("SHA256SUMS.txt has a bad line for {file}"))?;
        match found {
            Some(earlier) if earlier != digest => {
                return Err(format!(
                    "SHA256SUMS.txt lists two different checksums for {file}"
                ))
            }
            _ => found = Some(digest),
        }
    }
    found.ok_or_else(|| format!("SHA256SUMS.txt has no line for {file}"))
}

/// The bytes of the `spider-agent` entry in a gzipped tar archive.
///
/// Only a regular file at the archive root counts, written as `spider-agent`
/// or `./spider-agent`. Other entries are passed over, which is what lets an
/// archive made on macOS through, since its tar adds a `._spider-agent` entry
/// for extended attributes. An entry of that name that is a link, a second
/// entry of that name, an empty one, a header whose checksum does not add up,
/// and an archive that ends before its end marker are all refused.
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, String> {
    let tar = gunzip(archive)?;
    let mut offset = 0usize;
    let mut found: Option<Vec<u8>> = None;
    loop {
        let header = offset
            .checked_add(BLOCK)
            .and_then(|end| tar.get(offset..end))
            .ok_or("the archive is truncated: it ends without an end marker")?;
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        check_header_sum(header)?;
        let size = octal(field(header, 124, 136)?)
            .ok_or("the archive has an entry whose size cannot be read")?;
        let size = usize::try_from(size).map_err(|_| "an archive entry is too large")?;
        let name = entry_name(header)?;
        let kind = header.get(156).copied().unwrap_or(0);

        let start = offset + BLOCK;
        let end = start
            .checked_add(size)
            .ok_or("an archive entry is too large")?;
        let body = tar
            .get(start..end)
            .ok_or("the archive is truncated inside an entry")?;

        if name == BINARY_ENTRY {
            if kind != b'0' && kind != 0 {
                return Err(format!(
                    "{BINARY_ENTRY} in the archive is not a regular file"
                ));
            }
            if found.is_some() {
                return Err(format!("the archive holds {BINARY_ENTRY} twice"));
            }
            found = Some(body.to_vec());
        }

        let padded = size
            .div_ceil(BLOCK)
            .checked_mul(BLOCK)
            .ok_or("an archive entry is too large")?;
        offset = start
            .checked_add(padded)
            .ok_or("an archive entry is too large")?;
    }
    match found {
        Some(binary) if !binary.is_empty() => Ok(binary),
        Some(_) => Err(format!("{BINARY_ENTRY} in the archive is empty")),
        None => Err(format!("the archive holds no {BINARY_ENTRY} at its root")),
    }
}

fn gunzip(archive: &[u8]) -> Result<Vec<u8>, String> {
    let cap = u64::try_from(MAX_UNPACKED_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(archive)
        .take(cap)
        .read_to_end(&mut out)
        .map_err(|error| format!("the archive does not decompress: {error}"))?;
    if out.len() > MAX_UNPACKED_BYTES {
        return Err("the archive decompresses to more than this tool will hold".to_string());
    }
    Ok(out)
}

fn field(header: &[u8], start: usize, end: usize) -> Result<&[u8], String> {
    header
        .get(start..end)
        .ok_or_else(|| "the archive has a short header".to_string())
}

/// The sum of the header bytes with the checksum field read as spaces, which
/// is how tar computes it. A header that fails this is not a header.
fn check_header_sum(header: &[u8]) -> Result<(), String> {
    let stored = octal(field(header, 148, 156)?).ok_or("the archive has a damaged header")?;
    let computed: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    if stored == computed {
        Ok(())
    } else {
        Err("the archive has a damaged header".to_string())
    }
}

/// An octal number padded with spaces or NULs, as tar writes sizes and sums.
fn octal(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.trim_matches(|c: char| c == '\0' || c == ' ');
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

/// The entry path with a leading `./` taken off, joined to the ustar prefix
/// when there is one.
fn entry_name(header: &[u8]) -> Result<String, String> {
    let name = c_string(field(header, 0, 100)?);
    let ustar = field(header, 257, 262)? == b"ustar";
    let prefix = if ustar {
        c_string(field(header, 345, 500)?)
    } else {
        String::new()
    };
    let full = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    };
    Ok(full.strip_prefix("./").unwrap_or(&full).to_string())
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(bytes.get(..end).unwrap_or_default()).into_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use std::io::Write;

    /// One ustar header and its padded body.
    fn entry(name: &str, kind: u8, body: &[u8]) -> Vec<u8> {
        let mut header = [0u8; BLOCK];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(b"0000755");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        let size = format!("{:011o}", body.len());
        header[124..135].copy_from_slice(size.as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        let sum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(sum.as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(body);
        out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
        out
    }

    fn gzip(tar: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(tar).unwrap();
        encoder.finish().unwrap()
    }

    fn archive(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut tar: Vec<u8> = entries.concat();
        tar.extend_from_slice(&[0u8; BLOCK * 2]);
        gzip(&tar)
    }

    #[test]
    fn the_binary_comes_out_past_a_macos_attribute_entry() {
        let bytes = archive(&[
            entry("._spider-agent", b'0', b"attributes"),
            entry("./spider-agent", b'0', b"the binary"),
        ]);
        assert_eq!(extract_binary(&bytes).unwrap(), b"the binary");
    }

    #[test]
    fn an_archive_without_the_binary_is_refused() {
        let bytes = archive(&[entry("other", b'0', b"x")]);
        assert!(extract_binary(&bytes)
            .unwrap_err()
            .contains("no spider-agent"));
    }

    #[test]
    fn a_link_named_like_the_binary_is_refused() {
        let bytes = archive(&[entry("spider-agent", b'2', b"")]);
        assert!(extract_binary(&bytes)
            .unwrap_err()
            .contains("not a regular file"));
    }

    #[test]
    fn two_entries_named_like_the_binary_are_refused() {
        let bytes = archive(&[
            entry("spider-agent", b'0', b"one"),
            entry("spider-agent", b'0', b"two"),
        ]);
        assert!(extract_binary(&bytes).unwrap_err().contains("twice"));
    }

    #[test]
    fn a_tar_cut_short_is_refused_even_when_the_gzip_is_whole() {
        let tar = entry("spider-agent", b'0', &[7u8; 2000]);
        let cut = gzip(&tar[..1200]);
        assert!(extract_binary(&cut).unwrap_err().contains("truncated"));
        let no_end_marker = gzip(&tar);
        assert!(extract_binary(&no_end_marker)
            .unwrap_err()
            .contains("truncated"));
    }

    #[test]
    fn a_gzip_cut_short_or_garbage_is_refused() {
        let bytes = archive(&[entry("spider-agent", b'0', &[1u8; 4096])]);
        assert!(extract_binary(&bytes[..bytes.len() / 2]).is_err());
        assert!(extract_binary(b"not an archive at all").is_err());
    }

    #[test]
    fn a_damaged_header_is_refused() {
        let mut tar = entry("spider-agent", b'0', b"binary");
        tar[0] = b'x';
        tar.extend_from_slice(&[0u8; BLOCK * 2]);
        assert!(extract_binary(&gzip(&tar)).unwrap_err().contains("damaged"));
    }

    #[test]
    fn the_sums_file_is_read_the_way_shasum_writes_it() {
        let digest = sha256(b"payload");
        let sums = format!(
            "{}  other.tar.gz\n{}  wanted.tar.gz\n",
            hex(&sha256(b"other")),
            hex(&digest)
        );
        assert_eq!(
            expected_digest(sums.as_bytes(), "wanted.tar.gz").unwrap(),
            digest
        );
        let binary_mode = format!("{} *wanted.tar.gz\n", hex(&digest).to_uppercase());
        assert_eq!(
            expected_digest(binary_mode.as_bytes(), "wanted.tar.gz").unwrap(),
            digest
        );
        assert!(expected_digest(sums.as_bytes(), "missing.tar.gz").is_err());
        let torn = format!("{}  wanted.tar.gz\n", hex(&digest).get(..40).unwrap());
        assert!(expected_digest(torn.as_bytes(), "wanted.tar.gz").is_err());
        let split = format!(
            "{}  wanted.tar.gz\n{}  wanted.tar.gz\n",
            hex(&digest),
            hex(&sha256(b"else"))
        );
        assert!(expected_digest(split.as_bytes(), "wanted.tar.gz").is_err());
    }
}
