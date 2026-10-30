//! The maix store as a linkable region, so a sheet range, a document table and a slide table can
//! all be kept equal to the same block of the shared data layer.
//!
//! An [`Anchor::Maix`] names a rectangle (rows and columns inclusive) of one maix sheet at one
//! dimension vector. Cells read and write relative to its top-left corner. A cell cleared by a
//! sync is stored as `Null` (maix keeps history, so nothing is erased); `Null` reads as empty.

use std::collections::BTreeMap;

use alfadoc_model::{Anchor, Maix, Stamp, Value};

use crate::engine::{BridgeError, CellChange, Region, RegionAccess};

/// Read and write access to the cells of a [`Maix`], attributed to `author` at time `at`.
pub struct MaixAccess<'a> {
    maix: &'a mut Maix,
    author: String,
    at: u64,
}

impl<'a> MaixAccess<'a> {
    pub fn new(maix: &'a mut Maix, author: &str, at: u64) -> Self {
        MaixAccess { maix, author: author.to_string(), at }
    }
}

fn rect(anchor: &Anchor) -> Result<(u16, u32, u16, u32, u16, &alfadoc_model::DimVector), BridgeError> {
    let Anchor::Maix { sheet, top, left, bottom, right, dims } = anchor else {
        return Err(BridgeError::Unsupported("the data store is addressed by a maix anchor".into()));
    };
    if bottom < top || right < left {
        return Err(BridgeError::BadParams("the maix range is upside down".into()));
    }
    Ok((*sheet, *top, *left, *bottom, *right, dims))
}

impl RegionAccess for MaixAccess<'_> {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError> {
        let (sheet, top, left, bottom, right, dims) = rect(anchor)?;
        let sh = self.maix.sheet(sheet)?;
        let mut cells = BTreeMap::new();
        if let Some(dim) = self.maix.vectors.find(dims) {
            for (row, col, cell) in sh.slice(dim) {
                if row < top || row > bottom || col < left || col > right || cell.value == Value::Null {
                    continue;
                }
                cells.insert((row - top, col - left), cell.value.clone());
            }
        }
        Ok(Region { cells })
    }

    fn write_changes(&mut self, anchor: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError> {
        let (sheet, top, left, bottom, right, dims) = rect(anchor)?;
        let stamp = Stamp::new(&self.author, self.at);
        // Check everything first, so a bad change leaves the store untouched.
        let mut targets = Vec::with_capacity(changes.len());
        for ch in changes {
            let row = top.checked_add(ch.row).filter(|r| *r <= bottom);
            let col = left.checked_add(ch.col).filter(|c| *c <= right);
            let (Some(row), Some(col)) = (row, col) else {
                return Err(BridgeError::BadParams(format!("change at row {}, column {} is outside the linked range", ch.row, ch.col)));
            };
            targets.push((row, col));
        }
        for (ch, (row, col)) in changes.iter().zip(targets) {
            let value = ch.value.clone().unwrap_or(Value::Null);
            self.maix.set(sheet, row, col, dims, value, &stamp)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::DimVector;

    fn setup() -> (Maix, Anchor) {
        let mut m = Maix::default();
        let s = m.add_sheet("Costs");
        (m, Anchor::Maix { sheet: s, top: 2, left: 1, bottom: 4, right: 2, dims: DimVector::new() })
    }

    #[test]
    fn writes_are_relative_to_the_anchor_and_read_back() {
        let (mut m, a) = setup();
        let mut acc = MaixAccess::new(&mut m, "tester", 10);
        acc.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Int(5)) }, CellChange { row: 2, col: 1, value: Some(Value::Text("x".into())) }]).unwrap();
        let r = acc.read_region(&a).unwrap();
        assert_eq!(r.cells.get(&(0, 0)), Some(&Value::Int(5)));
        assert_eq!(r.cells.get(&(2, 1)), Some(&Value::Text("x".into())));
        // Stored at the absolute position.
        assert_eq!(m.get(1, 2, 1, &DimVector::new()).unwrap(), Some(&Value::Int(5)));
    }

    #[test]
    fn cells_outside_the_rectangle_are_not_read() {
        let (mut m, a) = setup();
        m.set(1, 0, 0, &DimVector::new(), Value::Int(1), &Stamp::new("t", 1)).unwrap();
        let acc = MaixAccess::new(&mut m, "t", 2);
        assert!(acc.read_region(&a).unwrap().cells.is_empty());
    }

    #[test]
    fn clearing_stores_null_and_reads_as_empty() {
        let (mut m, a) = setup();
        let mut acc = MaixAccess::new(&mut m, "t", 3);
        acc.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Int(9)) }]).unwrap();
        acc.write_changes(&a, &[CellChange { row: 0, col: 0, value: None }]).unwrap();
        assert!(acc.read_region(&a).unwrap().cells.is_empty());
    }

    #[test]
    fn out_of_range_changes_are_refused_and_change_nothing() {
        let (mut m, a) = setup();
        let mut acc = MaixAccess::new(&mut m, "t", 4);
        let bad = [CellChange { row: 0, col: 0, value: Some(Value::Int(1)) }, CellChange { row: 3, col: 0, value: Some(Value::Int(2)) }];
        assert!(matches!(acc.write_changes(&a, &bad), Err(BridgeError::BadParams(_))));
        assert!(acc.read_region(&a).unwrap().cells.is_empty());
    }

    #[test]
    fn other_anchors_are_unsupported() {
        let (mut m, _) = setup();
        let acc = MaixAccess::new(&mut m, "t", 5);
        assert!(matches!(acc.read_region(&Anchor::GridName { name: "n".into() }), Err(BridgeError::Unsupported(_))));
    }
}
