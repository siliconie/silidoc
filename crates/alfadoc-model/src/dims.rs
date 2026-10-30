//! Dimension vectors and their 64-bit hashes (maix spec §6.3).
//!
//! The canonical form sorts keys and joins `key=value` pairs with `|`. The hash is xxHash64 of
//! that string. **The empty vector hashes to 0** by definition: the maix 1.0 draft says "the hash
//! of the empty string", but xxHash64("") is not 0, so 1.1 states the rule outright (MAIX-1.1 E1).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh64::xxh64;

use crate::error::{Error, Result};

/// `role=estimator`, `time=2026-Q1`, … Sorted by key, so iteration order is canonical.
pub type DimVector = BTreeMap<String, String>;

pub fn canonical(v: &DimVector) -> String {
    let mut s = String::new();
    for (i, (k, val)) in v.iter().enumerate() {
        if i > 0 {
            s.push('|');
        }
        s.push_str(k);
        s.push('=');
        s.push_str(val);
    }
    s
}

pub fn dim_hash(v: &DimVector) -> u64 {
    if v.is_empty() { 0 } else { xxh64(canonical(v).as_bytes(), 0) }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimEntry {
    /// 0 when the first hash was free; otherwise the number of rehashes (spec §6.3.1).
    pub collision_counter: u8,
    pub pairs: DimVector,
}

/// The Dimension Vector Table: `dim_hash` → vector. Always resolve a cell's hash through this.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimTable {
    entries: BTreeMap<u64, DimEntry>,
}

fn hash_with_counter(canon: &str, counter: u8, hasher: &impl Fn(&str) -> u64) -> u64 {
    if counter == 0 { hasher(canon) } else { hasher(&format!("{canon}|__collision={counter}")) }
}

impl DimTable {
    pub fn resolve(&self, hash: u64) -> Option<&DimVector> {
        if hash == 0 { None } else { self.entries.get(&hash).map(|e| &e.pairs) }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every stored vector with its hash.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &DimVector)> {
        self.entries.iter().map(|(h, e)| (*h, &e.pairs))
    }

    /// Hashes of every stored vector that contains all of `filter`'s pairs (partial queries, §6.4).
    pub fn matching(&self, filter: &DimVector) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|(_, e)| filter.iter().all(|(k, v)| e.pairs.get(k) == Some(v)))
            .map(|(h, _)| *h)
            .collect()
    }

    /// The hash for `v`, adding it to the table if new.
    pub fn intern(&mut self, v: &DimVector) -> Result<u64> {
        self.intern_with(v, &|s| xxh64(s.as_bytes(), 0))
    }

    /// [`intern`](Self::intern) with an injectable hash function, so tests can force collisions.
    pub fn intern_with(&mut self, v: &DimVector, hasher: &impl Fn(&str) -> u64) -> Result<u64> {
        if v.is_empty() {
            return Ok(0);
        }
        let canon = canonical(v);
        for counter in 0..=u8::MAX {
            let h = hash_with_counter(&canon, counter, hasher);
            // 0 is reserved for the empty vector.
            if h == 0 {
                continue;
            }
            match self.entries.get(&h) {
                Some(e) if e.pairs == *v => return Ok(h),
                Some(_) => continue,
                None => {
                    self.entries.insert(h, DimEntry { collision_counter: counter, pairs: v.clone() });
                    return Ok(h);
                }
            }
        }
        Err(Error::DimCollisionOverflow)
    }

    /// The hash of `v` if it is already in the table (no insertion).
    pub fn find(&self, v: &DimVector) -> Option<u64> {
        if v.is_empty() {
            return Some(0);
        }
        let canon = canonical(v);
        let hasher = |s: &str| xxh64(s.as_bytes(), 0);
        for counter in 0..=u8::MAX {
            let h = hash_with_counter(&canon, counter, &hasher);
            match self.entries.get(&h) {
                Some(e) if e.pairs == *v => return Some(h),
                Some(_) => continue,
                None => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&str, &str)]) -> DimVector {
        pairs.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect()
    }

    #[test]
    fn canonical_is_sorted_and_stable() {
        let a = v(&[("time", "Q1"), ("role", "estimator")]);
        assert_eq!(canonical(&a), "role=estimator|time=Q1");
    }

    #[test]
    fn empty_vector_is_base_plane() {
        assert_eq!(dim_hash(&DimVector::new()), 0);
        let mut t = DimTable::default();
        assert_eq!(t.intern(&DimVector::new()).unwrap(), 0);
        assert!(t.is_empty());
    }

    #[test]
    fn intern_is_idempotent_and_resolvable() {
        let mut t = DimTable::default();
        let a = v(&[("role", "estimator"), ("scenario", "base")]);
        let h1 = t.intern(&a).unwrap();
        let h2 = t.intern(&a).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1, dim_hash(&a));
        assert_eq!(t.resolve(h1), Some(&a));
        assert_eq!(t.find(&a), Some(h1));
    }

    #[test]
    fn forced_collision_is_rehashed_and_both_resolve() {
        let mut t = DimTable::default();
        let a = v(&[("role", "a")]);
        let b = v(&[("role", "b")]);
        // A hasher that maps every base string to the same value; rehashed strings differ.
        let bad = |s: &str| if s.contains("__collision") { xxh64(s.as_bytes(), 1) } else { 42 };
        let ha = t.intern_with(&a, &bad).unwrap();
        let hb = t.intern_with(&b, &bad).unwrap();
        assert_eq!(ha, 42);
        assert_ne!(ha, hb);
        assert_eq!(t.resolve(ha), Some(&a));
        assert_eq!(t.resolve(hb), Some(&b));
        // Re-interning the colliding vector finds the same rehashed slot.
        assert_eq!(t.intern_with(&b, &bad).unwrap(), hb);
    }

    #[test]
    fn partial_query_matches_supersets() {
        let mut t = DimTable::default();
        let h1 = t.intern(&v(&[("role", "estimator"), ("time", "Q1")])).unwrap();
        let h2 = t.intern(&v(&[("role", "estimator"), ("time", "Q2")])).unwrap();
        let _ = t.intern(&v(&[("role", "cashier")])).unwrap();
        let mut got = t.matching(&v(&[("role", "estimator")]));
        got.sort();
        let mut want = vec![h1, h2];
        want.sort();
        assert_eq!(got, want);
    }
}
