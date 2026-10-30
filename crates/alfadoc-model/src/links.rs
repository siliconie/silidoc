//! Links: the connections between data in one place and data in another (LINKS-AND-SYNC.md).
//!
//! A link joins two [`Endpoint`]s. Each endpoint is a *target* (inside this package, or an
//! external file) plus an *anchor* that names a region using the host format's own mechanism, so
//! the anchor survives export to `.docx`, `.xlsx` and `.pptx`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dims::DimVector;
use crate::maix::Value;

/// How a region is named. Each variant maps to a native construct in its engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Anchor {
    /// A rectangle of the maix store: rows/cols inclusive, at a dimension vector.
    Maix { sheet: u16, top: u32, left: u16, bottom: u32, right: u16, #[serde(default)] dims: DimVector },
    /// A Word content control; its `tag` is `alfadoc:link=<link id>` (see [`link_tag`]).
    WordControl { tag: String },
    /// A spreadsheet defined name (named range) in an `.xlsx`.
    GridName { name: String },
    /// A slide shape, found by slide id and shape name (the shape also carries a link tag).
    DeckShape { slide_id: String, shape_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "where", rename_all = "snake_case")]
pub enum Target {
    /// A part inside this package (`document/main`, `slides/deck`, `data/maix`).
    Internal { part: String },
    /// A file elsewhere on disk or in cloud storage. `sha256` is the hash at last sync.
    External { path: String, sha256: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub target: Target,
    pub anchor: Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Changes flow both ways (three-way merge).
    Mirror,
    /// `a` is the source of truth; `b` is overwritten.
    AToB,
    /// `b` is the source of truth; `a` is overwritten.
    BToA,
}

/// The last-synced contents of a linked region: the "base" of the three-way merge.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub taken_at: u64,
    pub cells: Vec<SnapCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapCell {
    pub row: u32,
    pub col: u16,
    pub value: Value,
}

impl Snapshot {
    pub fn from_map(taken_at: u64, m: &BTreeMap<(u32, u16), Value>) -> Self {
        Snapshot { taken_at, cells: m.iter().map(|(&(row, col), v)| SnapCell { row, col, value: v.clone() }).collect() }
    }

    pub fn to_map(&self) -> BTreeMap<(u32, u16), Value> {
        self.cells.iter().map(|c| ((c.row, c.col), c.value.clone())).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub id: String,
    pub a: Endpoint,
    pub b: Endpoint,
    pub mode: SyncMode,
    #[serde(default)]
    pub base: Option<Snapshot>,
}

const TAG_PREFIX: &str = "alfadoc:link=";

/// The tag written into content controls, slide-shape tags and defined-name comments.
pub fn link_tag(link_id: &str) -> String {
    format!("{TAG_PREFIX}{link_id}")
}

/// The link id inside a tag, if it is one of ours.
pub fn parse_link_tag(tag: &str) -> Option<&str> {
    tag.strip_prefix(TAG_PREFIX).filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_round_trips_and_rejects_foreign_tags() {
        assert_eq!(parse_link_tag(&link_tag("L1")), Some("L1"));
        assert_eq!(parse_link_tag("alfadoc:link="), None);
        assert_eq!(parse_link_tag("zotero:item"), None);
    }

    #[test]
    fn snapshot_map_round_trip() {
        let mut m = BTreeMap::new();
        m.insert((1, 2), Value::Int(5));
        m.insert((0, 0), Value::Text("x".into()));
        let s = Snapshot::from_map(10, &m);
        assert_eq!(s.to_map(), m);
    }

    #[test]
    fn link_json_round_trip() {
        let l = Link {
            id: "L1".into(),
            a: Endpoint { target: Target::Internal { part: "data/maix".into() }, anchor: Anchor::Maix { sheet: 1, top: 0, left: 0, bottom: 4, right: 2, dims: DimVector::new() } },
            b: Endpoint { target: Target::External { path: "budget.xlsx".into(), sha256: "ab".into() }, anchor: Anchor::GridName { name: "Costs".into() } },
            mode: SyncMode::Mirror,
            base: None,
        };
        let back: Link = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(l, back);
    }
}
