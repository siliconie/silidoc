//! Syncing one [`Link`]: read both ends, merge against the last snapshot, write only what changed.

use std::collections::BTreeMap;

use alfadoc_model::{Link, Snapshot, SyncMode, Value};
use alfadoc_sync::{Conflict, Policy, merge3, one_way};

use crate::engine::{BridgeError, CellChange, RegionAccess};

type Grid = BTreeMap<(u32, u16), Value>;

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub wrote_a: usize,
    pub wrote_b: usize,
    pub conflicts: Vec<Conflict<(u32, u16), Value>>,
}

fn to_changes(w: Vec<((u32, u16), Option<Value>)>) -> Vec<CellChange> {
    w.into_iter().map(|((row, col), value)| CellChange { row, col, value }).collect()
}

/// Brings the two ends of `link` into agreement and advances its snapshot.
///
/// `a` and `b` are the access objects for the engines (or the maix store) that own each end; they
/// may be the same object when both ends live in one engine. If a write fails, nothing is
/// recorded: the snapshot is not advanced, so the next attempt re-merges from the same base.
pub fn sync_link(link: &mut Link, a: &mut dyn RegionAccess, b: &mut dyn RegionAccess, policy: Policy, now_ms: u64) -> Result<Outcome, BridgeError> {
    let ra: Grid = a.read_region(&link.a.anchor)?.cells;
    let rb: Grid = b.read_region(&link.b.anchor)?.cells;
    let base: Grid = link.base.as_ref().map(Snapshot::to_map).unwrap_or_default();

    let (write_a, write_b, result, conflicts) = match link.mode {
        SyncMode::Mirror => {
            let m = merge3(&base, &ra, &rb, policy);
            (m.write_a, m.write_b, m.result, m.conflicts)
        }
        SyncMode::AToB => (vec![], one_way(&ra, &rb), ra.clone(), vec![]),
        SyncMode::BToA => (one_way(&rb, &ra), vec![], rb.clone(), vec![]),
    };

    let outcome = Outcome { wrote_a: write_a.len(), wrote_b: write_b.len(), conflicts };
    if !write_a.is_empty() {
        a.write_changes(&link.a.anchor, &to_changes(write_a))?;
    }
    if !write_b.is_empty() {
        b.write_changes(&link.b.anchor, &to_changes(write_b))?;
    }
    link.base = Some(Snapshot::from_map(now_ms, &result));
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Region;
    use alfadoc_model::{Anchor, Endpoint, Target};

    #[derive(Default)]
    struct Mem {
        cells: Grid,
        fail_writes: bool,
        writes: usize,
    }
    impl RegionAccess for Mem {
        fn read_region(&self, _: &Anchor) -> Result<Region, BridgeError> {
            Ok(Region { cells: self.cells.clone() })
        }
        fn write_changes(&mut self, _: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError> {
            if self.fail_writes {
                return Err(BridgeError::Engine("locked".into()));
            }
            self.writes += changes.len();
            for c in changes {
                match &c.value {
                    Some(v) => self.cells.insert((c.row, c.col), v.clone()),
                    None => self.cells.remove(&(c.row, c.col)),
                };
            }
            Ok(())
        }
    }

    fn link(mode: SyncMode) -> Link {
        let ep = |n: &str| Endpoint { target: Target::Internal { part: n.into() }, anchor: Anchor::GridName { name: n.into() } };
        Link { id: "L".into(), a: ep("a"), b: ep("b"), mode, base: None }
    }

    fn mem(p: &[((u32, u16), i64)]) -> Mem {
        Mem { cells: p.iter().map(|(k, v)| (*k, Value::Int(*v))).collect(), ..Mem::default() }
    }

    #[test]
    fn edit_in_a_reaches_b_then_edit_in_b_reaches_a() {
        let mut l = link(SyncMode::Mirror);
        let (mut a, mut b) = (mem(&[((0, 0), 1), ((1, 0), 2)]), mem(&[]));
        let o = sync_link(&mut l, &mut a, &mut b, Policy::Manual, 1).unwrap();
        assert_eq!((o.wrote_a, o.wrote_b), (0, 2));
        assert_eq!(b.cells, a.cells);

        a.cells.insert((1, 0), Value::Int(20)); // user edits in A
        let o = sync_link(&mut l, &mut a, &mut b, Policy::Manual, 2).unwrap();
        assert_eq!((o.wrote_a, o.wrote_b), (0, 1));
        assert_eq!(b.cells[&(1, 0)], Value::Int(20));

        b.cells.insert((0, 0), Value::Int(10)); // user edits in B
        sync_link(&mut l, &mut a, &mut b, Policy::Manual, 3).unwrap();
        assert_eq!(a.cells[&(0, 0)], Value::Int(10));
    }

    #[test]
    fn a_second_sync_with_no_edits_writes_nothing() {
        let mut l = link(SyncMode::Mirror);
        let (mut a, mut b) = (mem(&[((0, 0), 1)]), mem(&[]));
        sync_link(&mut l, &mut a, &mut b, Policy::Manual, 1).unwrap();
        let before = (a.writes, b.writes);
        let o = sync_link(&mut l, &mut a, &mut b, Policy::Manual, 2).unwrap();
        assert_eq!((o.wrote_a, o.wrote_b), (0, 0));
        assert_eq!((a.writes, b.writes), before);
    }

    #[test]
    fn conflicts_are_reported_and_left_alone_under_manual() {
        let mut l = link(SyncMode::Mirror);
        let (mut a, mut b) = (mem(&[((0, 0), 1)]), mem(&[]));
        sync_link(&mut l, &mut a, &mut b, Policy::Manual, 1).unwrap();
        a.cells.insert((0, 0), Value::Int(2));
        b.cells.insert((0, 0), Value::Int(3));
        let o = sync_link(&mut l, &mut a, &mut b, Policy::Manual, 2).unwrap();
        assert_eq!(o.conflicts.len(), 1);
        assert_eq!((a.cells[&(0, 0)].clone(), b.cells[&(0, 0)].clone()), (Value::Int(2), Value::Int(3)));
        // Resolve by policy on the next round.
        let o = sync_link(&mut l, &mut a, &mut b, Policy::PreferA, 3).unwrap();
        assert!(o.conflicts.is_empty());
        assert_eq!(b.cells[&(0, 0)], Value::Int(2));
    }

    #[test]
    fn a_failed_write_does_not_advance_the_snapshot() {
        let mut l = link(SyncMode::Mirror);
        let (mut a, mut b) = (mem(&[((0, 0), 1)]), mem(&[]));
        b.fail_writes = true;
        assert!(sync_link(&mut l, &mut a, &mut b, Policy::Manual, 1).is_err());
        assert!(l.base.is_none());
        b.fail_writes = false; // the file was unlocked: the retry succeeds
        let o = sync_link(&mut l, &mut a, &mut b, Policy::Manual, 2).unwrap();
        assert_eq!(o.wrote_b, 1);
    }

    #[test]
    fn one_way_modes_overwrite_the_target_only() {
        let mut l = link(SyncMode::AToB);
        let (mut a, mut b) = (mem(&[((0, 0), 1)]), mem(&[((0, 0), 9), ((5, 5), 9)]));
        let o = sync_link(&mut l, &mut a, &mut b, Policy::Manual, 1).unwrap();
        assert_eq!((o.wrote_a, o.wrote_b), (0, 2));
        assert_eq!(b.cells, a.cells);

        let mut l = link(SyncMode::BToA);
        let (mut a, mut b) = (mem(&[((0, 0), 1)]), mem(&[((0, 0), 7)]));
        sync_link(&mut l, &mut a, &mut b, Policy::Manual, 1).unwrap();
        assert_eq!(a.cells[&(0, 0)], Value::Int(7));
    }
}
