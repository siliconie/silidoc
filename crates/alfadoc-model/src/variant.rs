//! Variant blocks (MAIX-1.1 §4): a content control or slide shape tagged
//! `alfadoc:variant={"audience":"client"}` is shown only while the active dimension vector
//! contains every pair of the tag. Content without such a tag is always shown.
//!
//! This module is the rule; showing and hiding is up to the editor that holds the content.

use crate::dims::DimVector;

const PREFIX: &str = "alfadoc:variant=";

/// The tag text for a variant.
pub fn variant_tag(variant: &DimVector) -> String {
    format!("{PREFIX}{}", serde_json::to_string(variant).unwrap_or_else(|_| "{}".to_string()))
}

/// The variant a tag names, or `None` when the tag is not a variant tag (or is damaged).
pub fn parse_variant_tag(tag: &str) -> Option<DimVector> {
    serde_json::from_str::<DimVector>(tag.strip_prefix(PREFIX)?).ok()
}

/// Whether a variant is visible under `active`: every pair of the variant is in `active`.
pub fn shown(variant: &DimVector, active: &DimVector) -> bool {
    variant.iter().all(|(k, v)| active.get(k) == Some(v))
}

/// Whether content carrying `tag` is visible under `active`. Other tags and no tag: visible.
/// A damaged variant tag is visible too, so nothing disappears because of a typo.
pub fn visible(tag: Option<&str>, active: &DimVector) -> bool {
    tag.and_then(parse_variant_tag).is_none_or(|variant| shown(&variant, active))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&str, &str)]) -> DimVector {
        pairs.iter().map(|(k, x)| ((*k).to_string(), (*x).to_string())).collect()
    }

    #[test]
    fn tags_round_trip() {
        let variant = v(&[("audience", "client"), ("language", "sw")]);
        assert_eq!(parse_variant_tag(&variant_tag(&variant)), Some(variant));
        assert_eq!(parse_variant_tag("alfadoc:link=L1"), None);
        assert_eq!(parse_variant_tag("alfadoc:variant={oops"), None);
    }

    #[test]
    fn a_variant_shows_when_the_active_vector_contains_it() {
        let client = v(&[("audience", "client")]);
        assert!(shown(&client, &v(&[("audience", "client"), ("language", "en")])));
        assert!(!shown(&client, &v(&[("audience", "internal")])));
        assert!(!shown(&client, &DimVector::new()));
        assert!(shown(&DimVector::new(), &DimVector::new()));
    }

    #[test]
    fn untagged_and_damaged_content_stays_visible() {
        let none = DimVector::new();
        assert!(visible(None, &none));
        assert!(visible(Some("alfadoc:link=L1"), &none));
        assert!(visible(Some("alfadoc:variant={oops"), &none));
        assert!(!visible(Some(&variant_tag(&v(&[("audience", "client")]))), &none));
    }
}
