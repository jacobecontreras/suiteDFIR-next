//! SHA-256 of files (ARCHITECTURE.md D18). ROADMAP C3 adds the progress/cancel variant and
//! `seal_tree`.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

const BUFFER_SIZE: usize = 1 << 20;

/// The lowercase hex SHA-256 of a file's contents, streamed through a read-only handle (the file
/// is never opened for writing, so evidence inputs can be hashed).
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Lowercase hex without a prefix (CONTRACTS.md §1).
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_of(contents: &[u8]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.bin");
        std::fs::write(&path, contents).unwrap();
        sha256_file(&path).unwrap()
    }

    // FIPS 180-2 / NIST CSRC example vectors.
    #[test]
    fn known_vectors() {
        assert_eq!(
            hash_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_of(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hash_of(&[b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn spans_buffer_boundaries() {
        // One byte past the buffer. Expected value from Python:
        // hashlib.sha256(b"a" * 2**20 + b"b").hexdigest()
        let mut contents = vec![b'a'; BUFFER_SIZE];
        contents.push(b'b');
        assert_eq!(
            hash_of(&contents),
            "371264331be3a89bb42c4fea3770469e9094f6ce8c8244b9ac2beb9ffd80e621"
        );
    }

    #[test]
    fn hashes_a_read_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.bin");
        std::fs::write(&path, b"abc").unwrap();
        crate::fsutil::set_read_only(&path).unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        crate::fsutil::test_support::make_writable(&path);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = sha256_file(&dir.path().join("nope")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }
}
