//! Passphrase protection for `.alfadoc` packages (MAIX-1.1 §13, with errata E7 and E8).
//!
//! * **Encryption.** [`encrypt`] wraps a whole package: the result is a small zip holding
//!   `security/encryption.json` (key derivation settings, salt, nonce) and `payload.enc`, the
//!   package encrypted with ChaCha20-Poly1305. This covers more than the namespace the spec asks
//!   for: the document, sheet and slide parts, the links and the title are all inside. A wrong
//!   passphrase and a changed file both fail the same way, because the cipher checks every byte.
//! * **Signing.** [`sign`] adds `security/signature.json`: an HMAC-SHA-256 over the name and
//!   SHA-256 of every other entry. The key comes from a passphrase and is **never stored in the
//!   file** (E8), so only someone who knows the passphrase can sign or verify. Saving the package
//!   again drops the signature, as it should: the content is no longer what was signed.
//!
//! The passphrase becomes a key with PBKDF2-HMAC-SHA-256 (hand-written on top of `sha2`, checked
//! against the published test vectors below).

use std::collections::BTreeMap;

use chacha20poly1305::ChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::container::{read_entries, sha256_hex, write_entries};
use crate::error::{Error, Result};

pub const ENCRYPTION_ENTRY: &str = "security/encryption.json";
pub const SIGNATURE_ENTRY: &str = "security/signature.json";
const PAYLOAD_ENTRY: &str = "payload.enc";

/// PBKDF2 rounds for new files.
pub const ITERATIONS: u32 = 300_000;
/// The most rounds a file may ask for, so a hostile file cannot stall the reader.
const MAX_ITERATIONS: u32 = 5_000_000;

#[derive(Debug, Serialize, Deserialize)]
struct EncryptionInfo {
    format: String,
    version: u32,
    kdf: String,
    iterations: u32,
    salt: String,
    cipher: String,
    nonce: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SignatureInfo {
    version: u32,
    signer: String,
    signed_at: u64,
    kdf: String,
    iterations: u32,
    salt: String,
    mac: String,
}

/// What checking a signature found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The package carries no signature.
    NotSigned,
    /// The signature matches: nothing changed since `signer` signed at `signed_at`.
    Valid { signer: String, signed_at: u64 },
    /// The signature does not match: the file was changed, or the passphrase is not the one used.
    Mismatch { signer: String },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out).map_err(|e| Error::Security(format!("no source of randomness: {e}")))?;
    Ok(out)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        let hashed = Sha256::digest(key);
        block[..32].copy_from_slice(&hashed[..]);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(&inner_hash[..]);
    let result = outer.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result[..]);
    out
}

/// PBKDF2-HMAC-SHA-256 for one 32-byte block.
fn pbkdf2(passphrase: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac_sha256(passphrase, &first);
    let mut t = u;
    for _ in 1..iterations {
        u = hmac_sha256(passphrase, &u);
        for (a, b) in t.iter_mut().zip(u.iter()) {
            *a ^= b;
        }
    }
    t
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `bytes` is a package protected by [`encrypt`].
pub fn is_encrypted(bytes: &[u8]) -> bool {
    read_entries(bytes).is_ok_and(|e| e.contains_key(ENCRYPTION_ENTRY))
}

/// A key made from a passphrase, kept so that saving does not repeat the slow derivation every
/// time. Every [`seal`](Protector::seal) uses a fresh random nonce.
#[derive(Clone)]
pub struct Protector {
    salt: Vec<u8>,
    key: [u8; 32],
    iterations: u32,
}

impl Protector {
    /// Derives the key (takes a moment, on purpose).
    pub fn new(passphrase: &str) -> Result<Self> {
        Self::with_iterations(passphrase, ITERATIONS)
    }

    fn with_iterations(passphrase: &str, iterations: u32) -> Result<Self> {
        if passphrase.is_empty() {
            return Err(Error::Security("the passphrase is empty".into()));
        }
        let salt: [u8; 16] = random()?;
        Ok(Protector { key: pbkdf2(passphrase.as_bytes(), &salt, iterations), salt: salt.to_vec(), iterations })
    }

    /// Encrypts a whole package.
    pub fn seal(&self, package: &[u8]) -> Result<Vec<u8>> {
        let nonce: [u8; 12] = random()?;
        let info = EncryptionInfo {
            format: "alfadoc-encrypted".into(),
            version: 1,
            kdf: "pbkdf2-hmac-sha256".into(),
            iterations: self.iterations,
            salt: hex(&self.salt),
            cipher: "chacha20poly1305".into(),
            nonce: hex(&nonce),
        };
        let info_bytes = serde_json::to_vec_pretty(&info)?;
        let sealed = ChaCha20Poly1305::new(&self.key.into())
            .encrypt(&nonce.into(), Payload { msg: package, aad: &info_bytes })
            .map_err(|_| Error::Security("encryption failed".into()))?;
        let mut entries = BTreeMap::new();
        entries.insert(ENCRYPTION_ENTRY.to_string(), info_bytes);
        entries.insert(PAYLOAD_ENTRY.to_string(), sealed);
        write_entries(&entries)
    }
}

/// Encrypts a whole package with a passphrase.
pub fn encrypt(package: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    Protector::new(passphrase)?.seal(package)
}

/// The package inside an encrypted file, and the key that opened it (so a later save can seal
/// the package again the same way). A wrong passphrase and a damaged file both give
/// [`Error::WrongPassphrase`].
pub fn open(bytes: &[u8], passphrase: &str) -> Result<(Vec<u8>, Protector)> {
    let entries = read_entries(bytes)?;
    let info_bytes = entries.get(ENCRYPTION_ENTRY).ok_or(Error::MissingEntry("security/encryption.json"))?;
    let sealed = entries.get(PAYLOAD_ENTRY).ok_or(Error::MissingEntry("payload.enc"))?;
    let info: EncryptionInfo = serde_json::from_slice(info_bytes)?;
    if info.format != "alfadoc-encrypted" || info.version != 1 || info.kdf != "pbkdf2-hmac-sha256" || info.cipher != "chacha20poly1305" {
        return Err(Error::Security("this protection method is not supported".into()));
    }
    if info.iterations == 0 || info.iterations > MAX_ITERATIONS {
        return Err(Error::Security("the key settings in this file are out of range".into()));
    }
    let salt = unhex(&info.salt).ok_or_else(|| Error::Security("damaged salt".into()))?;
    let nonce: [u8; 12] = unhex(&info.nonce).and_then(|n| n.try_into().ok()).ok_or_else(|| Error::Security("damaged nonce".into()))?;
    let key = pbkdf2(passphrase.as_bytes(), &salt, info.iterations);
    let plain = ChaCha20Poly1305::new(&key.into()).decrypt(&nonce.into(), Payload { msg: sealed, aad: info_bytes }).map_err(|_| Error::WrongPassphrase)?;
    Ok((plain, Protector { salt, key, iterations: info.iterations }))
}

/// [`open`] without keeping the key.
pub fn decrypt(bytes: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    open(bytes, passphrase).map(|(plain, _)| plain)
}

#[cfg(test)]
fn encrypt_with(package: &[u8], passphrase: &str, iterations: u32) -> Result<Vec<u8>> {
    Protector::with_iterations(passphrase, iterations)?.seal(package)
}

/// The text the signature covers: every entry but the signature itself, by name and SHA-256.
fn signed_text(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = String::new();
    for (name, bytes) in entries {
        if name == SIGNATURE_ENTRY || name == "mimetype" {
            continue;
        }
        out.push_str(name);
        out.push('\0');
        out.push_str(&sha256_hex(bytes));
        out.push('\n');
    }
    out.into_bytes()
}

/// Signs a (not encrypted) package with a passphrase.
pub fn sign(package: &[u8], signer: &str, passphrase: &str, now_ms: u64) -> Result<Vec<u8>> {
    sign_with(package, signer, passphrase, now_ms, ITERATIONS)
}

fn sign_with(package: &[u8], signer: &str, passphrase: &str, now_ms: u64, iterations: u32) -> Result<Vec<u8>> {
    if passphrase.is_empty() {
        return Err(Error::Security("the passphrase is empty".into()));
    }
    let mut entries = read_entries(package)?;
    if entries.contains_key(ENCRYPTION_ENTRY) {
        return Err(Error::Security("an encrypted file cannot be signed; sign the unprotected copy".into()));
    }
    entries.remove(SIGNATURE_ENTRY);
    let salt: [u8; 16] = random()?;
    let key = pbkdf2(passphrase.as_bytes(), &salt, iterations);
    let mac = hmac_sha256(&key, &signed_text(&entries));
    let info = SignatureInfo { version: 1, signer: signer.trim().to_string(), signed_at: now_ms, kdf: "pbkdf2-hmac-sha256".into(), iterations, salt: hex(&salt), mac: hex(&mac) };
    entries.insert(SIGNATURE_ENTRY.to_string(), serde_json::to_vec_pretty(&info)?);
    write_entries(&entries)
}

/// Checks a package's signature with the passphrase it was signed with.
pub fn verify(package: &[u8], passphrase: &str) -> Result<Verdict> {
    let entries = read_entries(package)?;
    let Some(raw) = entries.get(SIGNATURE_ENTRY) else {
        return Ok(Verdict::NotSigned);
    };
    let info: SignatureInfo = serde_json::from_slice(raw)?;
    if info.version != 1 || info.kdf != "pbkdf2-hmac-sha256" || info.iterations == 0 || info.iterations > MAX_ITERATIONS {
        return Err(Error::Security("this signature method is not supported".into()));
    }
    let salt = unhex(&info.salt).ok_or_else(|| Error::Security("damaged salt".into()))?;
    let stored = unhex(&info.mac).ok_or_else(|| Error::Security("damaged signature".into()))?;
    let key = pbkdf2(passphrase.as_bytes(), &salt, info.iterations);
    let mac = hmac_sha256(&key, &signed_text(&entries));
    Ok(if same(&mac, &stored) { Verdict::Valid { signer: info.signer, signed_at: info.signed_at } } else { Verdict::Mismatch { signer: info.signer } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::Alfadoc;

    const FAST: u32 = 50;

    fn package() -> Vec<u8> {
        let mut d = Alfadoc::new("00000000-0000-4000-8000-000000000009", "Secret plan", 5);
        d.parts.insert("document/main.docx".into(), b"the quick brown fox".to_vec());
        d.to_bytes().unwrap()
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn pbkdf2_matches_the_published_vectors() {
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 1)), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 2)), "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 4096)), "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn an_encrypted_package_opens_only_with_its_passphrase() {
        let plain = package();
        let sealed = encrypt_with(&plain, "correct horse", FAST).unwrap();
        assert!(is_encrypted(&sealed));
        assert!(!is_encrypted(&plain));
        assert!(!sealed.windows(9).any(|w| w == b"the quick"), "the content is not readable in the file");
        assert!(matches!(Alfadoc::from_bytes(&sealed), Err(Error::Encrypted)));
        assert!(matches!(decrypt(&sealed, "wrong"), Err(Error::WrongPassphrase)));
        assert_eq!(decrypt(&sealed, "correct horse").unwrap(), plain);
        assert_eq!(Alfadoc::from_bytes(&decrypt(&sealed, "correct horse").unwrap()).unwrap().manifest.title, "Secret plan");
    }

    #[test]
    fn a_kept_key_seals_again_without_the_passphrase() {
        let sealed = encrypt_with(&package(), "pw", FAST).unwrap();
        let (plain, keeper) = open(&sealed, "pw").unwrap();
        let again = keeper.seal(&plain).unwrap();
        assert_ne!(again, sealed, "a fresh nonce each time");
        assert_eq!(decrypt(&again, "pw").unwrap(), plain);
        assert!(matches!(decrypt(&again, "other"), Err(Error::WrongPassphrase)));
    }

    #[test]
    fn every_encryption_is_different_and_tampering_is_caught() {
        let plain = package();
        let (a, b) = (encrypt_with(&plain, "pw", FAST).unwrap(), encrypt_with(&plain, "pw", FAST).unwrap());
        assert_ne!(a, b, "fresh salt and nonce each time");
        // Flip a byte of the sealed payload.
        let mut entries = read_entries(&a).unwrap();
        if let Some(p) = entries.get_mut(PAYLOAD_ENTRY) {
            let mid = p.len() / 2;
            p[mid] ^= 1;
        }
        assert!(matches!(decrypt(&write_entries(&entries).unwrap(), "pw"), Err(Error::WrongPassphrase)));
        // Changing the key settings is caught too (they are authenticated).
        let mut entries = read_entries(&a).unwrap();
        if let Some(info) = entries.get_mut(ENCRYPTION_ENTRY) {
            let text = String::from_utf8(info.clone()).unwrap();
            *info = text.replace(&format!("\"iterations\": {FAST}"), &format!("\"iterations\": {}", FAST + 1)).into_bytes();
        }
        assert!(decrypt(&write_entries(&entries).unwrap(), "pw").is_err());
    }

    #[test]
    fn hostile_settings_are_refused_quickly() {
        let sealed = encrypt_with(&package(), "pw", FAST).unwrap();
        let mut entries = read_entries(&sealed).unwrap();
        if let Some(info) = entries.get_mut(ENCRYPTION_ENTRY) {
            *info = String::from_utf8(info.clone()).unwrap().replace(&format!("\"iterations\": {FAST}"), "\"iterations\": 4000000000").into_bytes();
        }
        assert!(matches!(decrypt(&write_entries(&entries).unwrap(), "pw"), Err(Error::Security(_))));
        assert!(encrypt_with(&package(), "", FAST).is_err());
    }

    #[test]
    fn a_signature_holds_until_the_file_changes() {
        let plain = package();
        assert_eq!(verify(&plain, "pw").unwrap(), Verdict::NotSigned);
        let signed = sign_with(&plain, " Elia ", "pw", 123, FAST).unwrap();
        assert_eq!(verify(&signed, "pw").unwrap(), Verdict::Valid { signer: "Elia".into(), signed_at: 123 });
        assert_eq!(verify(&signed, "other").unwrap(), Verdict::Mismatch { signer: "Elia".into() });
        // The signed package still opens as an ordinary one.
        assert_eq!(Alfadoc::from_bytes(&signed).unwrap().manifest.title, "Secret plan");
        // Change a part and the signature no longer matches.
        let mut entries = read_entries(&signed).unwrap();
        entries.insert("document/main.docx".into(), b"the slow brown fox".to_vec());
        assert_eq!(verify(&write_entries(&entries).unwrap(), "pw").unwrap(), Verdict::Mismatch { signer: "Elia".into() });
        // Signing again replaces the old signature.
        let again = sign_with(&signed, "Amina", "pw2", 456, FAST).unwrap();
        assert_eq!(verify(&again, "pw2").unwrap(), Verdict::Valid { signer: "Amina".into(), signed_at: 456 });
    }

    #[test]
    fn an_encrypted_file_cannot_be_signed() {
        let sealed = encrypt_with(&package(), "pw", FAST).unwrap();
        assert!(sign_with(&sealed, "x", "pw", 1, FAST).is_err());
    }
}
