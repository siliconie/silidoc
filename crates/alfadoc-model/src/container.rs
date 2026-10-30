//! The `.alfadoc` package (ALFADOC-FORMAT.md): a zip with a fixed entry layout.
//!
//! ```text
//! mimetype            "application/vnd.alfadoc+zip"   (first entry, stored uncompressed)
//! manifest.json       identity, versions, part list
//! data/maix.json      the maix store (JSON-records profile)
//! links.json          the link registry
//! document/…          word-processor parts (WordCraft model)
//! sheets/…            spreadsheet parts (GridCraft model)
//! slides/…            presentation parts (DeckCraft model)
//! assets/…            pictures and other binaries
//! ```
//!
//! Reading is bounded: entry names are validated, entry count and sizes are capped, and sizes
//! claimed by the zip header are never trusted (reads are limited while streaming).

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::error::{Error, Result};
use crate::links::Link;
use crate::maix::Maix;

pub const MIMETYPE: &str = "application/vnd.alfadoc+zip";
pub const FORMAT_VERSION: &str = "0.1";

pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: String,
    /// Stable identity of this document; the caller supplies it (a UUID string).
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    /// `name version` of the writer, informational.
    pub generator: String,
}

impl Manifest {
    pub fn new(id: &str, title: &str, now_ms: u64) -> Self {
        Manifest {
            format: "alfadoc".into(),
            version: FORMAT_VERSION.into(),
            id: id.into(),
            title: title.into(),
            created_at: now_ms,
            updated_at: now_ms,
            generator: concat!("alfadoc ", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

/// An open `.alfadoc`. `parts` holds `document/…`, `slides/…` and `assets/…` entries as opaque bytes:
/// the engines own their content, this crate only stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct Alfadoc {
    pub manifest: Manifest,
    pub maix: Maix,
    pub links: Vec<Link>,
    pub parts: BTreeMap<String, Vec<u8>>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Package entry names must be relative, forward-slashed, and free of `..`, `.`, empty
/// components, drive letters and control characters (zip-slip hardening).
pub fn validate_entry_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(|c| c.is_control())
        || name.split('/').any(|p| p.is_empty() || p == "." || p == "..");
    if bad { Err(Error::BadEntryName(name.to_string())) } else { Ok(()) }
}

const RESERVED: [&str; 4] = ["mimetype", "manifest.json", "data/maix.json", "links.json"];

fn is_part_name(name: &str) -> bool {
    name.starts_with("document/") || name.starts_with("sheets/") || name.starts_with("slides/") || name.starts_with("assets/")
}

impl Alfadoc {
    pub fn new(id: &str, title: &str, now_ms: u64) -> Self {
        Alfadoc { manifest: Manifest::new(id, title, now_ms), maix: Maix::default(), links: vec![], parts: BTreeMap::new() }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored)?;
        zip.write_all(MIMETYPE.as_bytes())?;
        zip.start_file("manifest.json", deflated)?;
        zip.write_all(&serde_json::to_vec_pretty(&self.manifest)?)?;
        zip.start_file("data/maix.json", deflated)?;
        zip.write_all(&serde_json::to_vec(&self.maix)?)?;
        zip.start_file("links.json", deflated)?;
        zip.write_all(&serde_json::to_vec_pretty(&self.links)?)?;
        for (name, bytes) in &self.parts {
            validate_entry_name(name)?;
            if !is_part_name(name) {
                return Err(Error::BadEntryName(name.clone()));
            }
            zip.start_file(name.as_str(), deflated)?;
            zip.write_all(bytes)?;
        }
        Ok(zip.finish()?.into_inner())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        if zip.len() > MAX_ENTRIES {
            return Err(Error::Limit(format!("{} entries (max {MAX_ENTRIES})", zip.len())));
        }
        let mut total: u64 = 0;
        let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for i in 0..zip.len() {
            let file = zip.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_string();
            validate_entry_name(&name)?;
            let mut buf = Vec::new();
            // Never trust the header's size: stop at the cap while streaming.
            file.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
            if buf.len() as u64 > MAX_ENTRY_BYTES {
                return Err(Error::Limit(format!("entry {name} exceeds {MAX_ENTRY_BYTES} bytes")));
            }
            total = total.saturating_add(buf.len() as u64);
            if total > MAX_TOTAL_BYTES {
                return Err(Error::Limit(format!("package exceeds {MAX_TOTAL_BYTES} bytes uncompressed")));
            }
            entries.insert(name, buf);
        }

        match entries.get("mimetype") {
            Some(m) if m.as_slice() == MIMETYPE.as_bytes() => {}
            Some(_) => return Err(Error::NotAlfadoc("wrong mimetype".into())),
            None => return Err(Error::MissingEntry("mimetype")),
        }
        let manifest: Manifest = serde_json::from_slice(entries.get("manifest.json").ok_or(Error::MissingEntry("manifest.json"))?)?;
        if manifest.format != "alfadoc" {
            return Err(Error::NotAlfadoc(format!("format is {:?}", manifest.format)));
        }
        let maix: Maix = serde_json::from_slice(entries.get("data/maix.json").ok_or(Error::MissingEntry("data/maix.json"))?)?;
        let links: Vec<Link> = match entries.get("links.json") {
            Some(b) => serde_json::from_slice(b)?,
            None => vec![],
        };
        let parts = entries.into_iter().filter(|(n, _)| !RESERVED.contains(&n.as_str()) && is_part_name(n)).collect();
        Ok(Alfadoc { manifest, maix, links, parts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dims::DimVector;
    use crate::links::{Anchor, Endpoint, SyncMode, Target};
    use crate::maix::{Stamp, Value};

    fn sample() -> Alfadoc {
        let mut d = Alfadoc::new("00000000-0000-4000-8000-000000000001", "Budget report", 1_000);
        let s = d.maix.add_sheet("Costs");
        d.maix.set(s, 0, 0, &DimVector::new(), Value::Text("Item".into()), &Stamp::new("test", 1)).unwrap();
        d.maix.set(s, 1, 0, &DimVector::new(), Value::Int(4500), &Stamp::new("test", 2)).unwrap();
        d.parts.insert("document/main.docx".into(), b"fake-docx".to_vec());
        d.parts.insert("assets/logo.png".into(), vec![0, 1, 2, 3]);
        d.links.push(Link {
            id: "L1".into(),
            a: Endpoint { target: Target::Internal { part: "data/maix".into() }, anchor: Anchor::Maix { sheet: s, top: 0, left: 0, bottom: 1, right: 0, dims: DimVector::new() } },
            b: Endpoint { target: Target::Internal { part: "document/main.docx".into() }, anchor: Anchor::WordControl { tag: "alfadoc:link=L1".into() } },
            mode: SyncMode::Mirror,
            base: None,
        });
        d
    }

    #[test]
    fn round_trip_preserves_everything() {
        let d = sample();
        let bytes = d.to_bytes().unwrap();
        assert_eq!(Alfadoc::from_bytes(&bytes).unwrap(), d);
    }

    #[test]
    fn mimetype_is_the_first_stored_entry() {
        let bytes = sample().to_bytes().unwrap();
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
        // Stored entry: the mimetype text appears uncompressed right after the 30-byte header + name.
        let head = &bytes[..120];
        assert!(head.windows(MIMETYPE.len()).any(|w| w == MIMETYPE.as_bytes()));
    }

    #[test]
    fn rejects_non_alfadoc_zip() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("hello.txt", SimpleFileOptions::default()).unwrap();
        zip.write_all(b"hi").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(matches!(Alfadoc::from_bytes(&bytes), Err(Error::MissingEntry("mimetype"))));
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(Alfadoc::from_bytes(b"not a zip at all").is_err());
        assert!(Alfadoc::from_bytes(&[]).is_err());
    }

    #[test]
    fn rejects_zip_slip_names_on_read() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("../evil", SimpleFileOptions::default()).unwrap();
        zip.write_all(b"x").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(matches!(Alfadoc::from_bytes(&bytes), Err(Error::BadEntryName(_))));
    }

    #[test]
    fn entry_name_rules() {
        for ok in ["document/main.docx", "assets/a/b.png", "slides/deck.pptx"] {
            assert!(validate_entry_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/abs", "a/../b", "a//b", "./a", "a\\b", "C:/x", "a/\u{0}b", "a/"] {
            assert!(validate_entry_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn writer_refuses_parts_outside_the_known_folders() {
        let mut d = sample();
        d.parts.insert("manifest.json".into(), vec![]);
        assert!(d.to_bytes().is_err());
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
