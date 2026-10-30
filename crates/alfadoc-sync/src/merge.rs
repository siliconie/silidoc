//! Three-way merge of two edited copies of the same region against the last-synced snapshot.
//!
//! For each key, with `base` the snapshot and `a`/`b` the current contents of the two ends:
//! - `a == b`: nothing to do.
//! - only one side changed since `base`: that side wins and is copied to the other.
//! - both changed, differently: a [`Conflict`], settled by the [`Policy`].
//!
//! `None` means "absent", so deletions merge like any other change.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Side A wins every conflict.
    PreferA,
    /// Side B wins every conflict.
    PreferB,
    /// Leave conflicting keys untouched on both sides and report them; the user decides.
    Manual,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Conflict<K, V> {
    pub key: K,
    pub base: Option<V>,
    pub a: Option<V>,
    pub b: Option<V>,
}

/// What to write where. `write_a` / `write_b` are `(key, new value or None to delete)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Merge<K, V> {
    pub write_a: Vec<(K, Option<V>)>,
    pub write_b: Vec<(K, Option<V>)>,
    /// The agreed contents after the writes: becomes the next snapshot. Unresolved conflicts keep
    /// their `base` value here, so they stay conflicts next time.
    pub result: BTreeMap<K, V>,
    pub conflicts: Vec<Conflict<K, V>>,
}

pub fn merge3<K: Ord + Clone, V: Clone + PartialEq>(base: &BTreeMap<K, V>, a: &BTreeMap<K, V>, b: &BTreeMap<K, V>, policy: Policy) -> Merge<K, V> {
    let mut keys: Vec<&K> = base.keys().chain(a.keys()).chain(b.keys()).collect();
    keys.sort();
    keys.dedup();

    let mut out = Merge { write_a: vec![], write_b: vec![], result: BTreeMap::new(), conflicts: vec![] };
    for k in keys {
        let (vb, va, vbb) = (base.get(k), a.get(k), b.get(k));
        let settled: Option<Option<&V>> = if va == vbb {
            Some(va)
        } else if va == vb {
            out.write_a.push((k.clone(), vbb.cloned()));
            Some(vbb)
        } else if vbb == vb {
            out.write_b.push((k.clone(), va.cloned()));
            Some(va)
        } else {
            match policy {
                Policy::PreferA => {
                    out.write_b.push((k.clone(), va.cloned()));
                    Some(va)
                }
                Policy::PreferB => {
                    out.write_a.push((k.clone(), vbb.cloned()));
                    Some(vbb)
                }
                Policy::Manual => {
                    out.conflicts.push(Conflict { key: k.clone(), base: vb.cloned(), a: va.cloned(), b: vbb.cloned() });
                    Some(vb)
                }
            }
        };
        if let Some(Some(v)) = settled {
            out.result.insert(k.clone(), v.clone());
        }
    }
    out
}

/// One-way sync: make `target` equal `source`. Returns the writes for `target`.
pub fn one_way<K: Ord + Clone, V: Clone + PartialEq>(source: &BTreeMap<K, V>, target: &BTreeMap<K, V>) -> Vec<(K, Option<V>)> {
    let mut w = vec![];
    for (k, v) in source {
        if target.get(k) != Some(v) {
            w.push((k.clone(), Some(v.clone())));
        }
    }
    for k in target.keys() {
        if !source.contains_key(k) {
            w.push((k.clone(), None));
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &[(u8, i32)]) -> BTreeMap<u8, i32> {
        p.iter().copied().collect()
    }

    #[test]
    fn one_sided_changes_propagate_both_ways() {
        let base = m(&[(1, 10), (2, 20)]);
        let a = m(&[(1, 11), (2, 20)]); // A edited key 1
        let b = m(&[(1, 10), (2, 22)]); // B edited key 2
        let r = merge3(&base, &a, &b, Policy::Manual);
        assert_eq!(r.write_b, vec![(1, Some(11))]);
        assert_eq!(r.write_a, vec![(2, Some(22))]);
        assert_eq!(r.result, m(&[(1, 11), (2, 22)]));
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn same_change_on_both_sides_is_not_a_conflict() {
        let base = m(&[(1, 10)]);
        let r = merge3(&base, &m(&[(1, 12)]), &m(&[(1, 12)]), Policy::Manual);
        assert!(r.write_a.is_empty() && r.write_b.is_empty() && r.conflicts.is_empty());
        assert_eq!(r.result, m(&[(1, 12)]));
    }

    #[test]
    fn real_conflict_follows_policy() {
        let base = m(&[(1, 10)]);
        let (a, b) = (m(&[(1, 11)]), m(&[(1, 12)]));
        let pa = merge3(&base, &a, &b, Policy::PreferA);
        assert_eq!(pa.write_b, vec![(1, Some(11))]);
        assert!(pa.write_a.is_empty());
        let pb = merge3(&base, &a, &b, Policy::PreferB);
        assert_eq!(pb.write_a, vec![(1, Some(12))]);
        let man = merge3(&base, &a, &b, Policy::Manual);
        assert!(man.write_a.is_empty() && man.write_b.is_empty());
        assert_eq!(man.conflicts.len(), 1);
        assert_eq!(man.result, m(&[(1, 10)])); // base kept, stays a conflict next round
    }

    #[test]
    fn deletion_vs_edit_is_a_conflict_and_deletion_alone_propagates() {
        let base = m(&[(1, 10), (2, 20)]);
        // A deletes key 1 (B untouched): propagates. A deletes key 2 while B edits it: conflict.
        let a = m(&[]);
        let b = m(&[(1, 10), (2, 21)]);
        let r = merge3(&base, &a, &b, Policy::Manual);
        assert_eq!(r.write_b, vec![(1, None)]);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].key, 2);
    }

    #[test]
    fn first_sync_with_empty_base_copies_disjoint_keys() {
        let r = merge3(&m(&[]), &m(&[(1, 1)]), &m(&[(2, 2)]), Policy::Manual);
        assert_eq!(r.write_b, vec![(1, Some(1))]);
        assert_eq!(r.write_a, vec![(2, Some(2))]);
        assert_eq!(r.result, m(&[(1, 1), (2, 2)]));
    }

    #[test]
    fn merging_twice_is_stable() {
        let base = m(&[(1, 10), (2, 20)]);
        let a = m(&[(1, 11), (2, 20)]);
        let b = m(&[(1, 10), (2, 22)]);
        let first = merge3(&base, &a, &b, Policy::Manual);
        let again = merge3(&first.result, &first.result, &first.result, Policy::Manual);
        assert!(again.write_a.is_empty() && again.write_b.is_empty() && again.conflicts.is_empty());
    }

    #[test]
    fn one_way_makes_target_equal_source() {
        let w = one_way(&m(&[(1, 1), (2, 2)]), &m(&[(2, 9), (3, 3)]));
        assert_eq!(w, vec![(1, Some(1)), (2, Some(2)), (3, None)]);
        assert!(one_way(&m(&[(1, 1)]), &m(&[(1, 1)])).is_empty());
    }
}
