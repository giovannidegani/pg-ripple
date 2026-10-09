//! sh:equals, sh:disjoint, and numeric range (sh:min/maxExclusive/Inclusive) constraint
//! checkers (v0.45.0 / v0.48.0).
//!
//! Both constraints compare the *set* of value-node IDs for the focus node's
//! declared path with the set of value-node IDs for an *other* path.
//!
//! ## sh:equals
//! For every focus node n:
//!   values(path) == values(other_path)
//! Implemented as two NOT-EXISTS checks (one per direction).
//!
//! ## sh:disjoint
//! For every focus node n:
//!   values(path) ∩ values(other_path) == ∅
//! Implemented as an EXISTS check for any shared value ID.

use super::{ConstraintArgs, Violation, compare_dictionary_values, get_value_ids};

/// Check `sh:equals other_path_iri` — the value set for the focus node's
/// declared path must be identical to the value set for `other_path_iri`.
pub(crate) fn check_equals(
    other_path_iri: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
) {
    let other_pred_id = match crate::dictionary::lookup_iri(other_path_iri) {
        Some(id) => id,
        None => {
            // Other path not in dictionary → other set is empty.
            // Our set is non-empty only if we have values; if we do, that is a
            // violation (the sets are not equal).
            let my_values = get_value_ids(args.focus, args.path_id, args.graph_id);
            if !my_values.is_empty() {
                let focus_iri = crate::shacl::decode_id_safe(args.focus);
                violations.push(Violation {
                    focus_node: focus_iri,
                    shape_iri: args.shape_iri.to_owned(),
                    path: Some(args.path_iri.to_owned()),
                    constraint: "sh:equals".to_owned(),
                    message: format!(
                        "value set for <{}> is not equal to value set for <{other_path_iri}>: \
                         other path has no values",
                        args.path_iri
                    ),
                    severity: "Violation".to_owned(),
                    sh_value: None,
                    sh_source_constraint_component: None,
                });
            }
            return;
        }
    };

    let my_values: std::collections::HashSet<i64> =
        get_value_ids(args.focus, args.path_id, args.graph_id)
            .into_iter()
            .collect();
    let other_values: std::collections::HashSet<i64> =
        get_value_ids(args.focus, other_pred_id, args.graph_id)
            .into_iter()
            .collect();

    if my_values != other_values {
        let focus_iri = crate::shacl::decode_id_safe(args.focus);
        // Describe the symmetric difference to aid debugging.
        let only_mine: Vec<i64> = my_values.difference(&other_values).copied().collect();
        let only_other: Vec<i64> = other_values.difference(&my_values).copied().collect();
        violations.push(Violation {
            focus_node: focus_iri,
            shape_iri: args.shape_iri.to_owned(),
            path: Some(args.path_iri.to_owned()),
            constraint: "sh:equals".to_owned(),
            message: format!(
                "value set for <{}> != value set for <{other_path_iri}>: \
                 only-in-path={only_mine:?}, only-in-other={only_other:?}",
                args.path_iri
            ),
            severity: "Violation".to_owned(),
            sh_value: None,
            sh_source_constraint_component: None,
        });
    }
}

/// Check `sh:disjoint other_path_iri` — the value set for the focus node's
/// declared path must share no common values with `other_path_iri`.
pub(crate) fn check_disjoint(
    other_path_iri: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
) {
    let other_pred_id = match crate::dictionary::lookup_iri(other_path_iri) {
        Some(id) => id,
        None => {
            // Other path not in dictionary → other set is empty → disjoint trivially.
            return;
        }
    };

    let my_values: std::collections::HashSet<i64> =
        get_value_ids(args.focus, args.path_id, args.graph_id)
            .into_iter()
            .collect();
    let other_values: std::collections::HashSet<i64> =
        get_value_ids(args.focus, other_pred_id, args.graph_id)
            .into_iter()
            .collect();

    let shared: Vec<i64> = my_values.intersection(&other_values).copied().collect();
    if !shared.is_empty() {
        let focus_iri = crate::shacl::decode_id_safe(args.focus);
        violations.push(Violation {
            focus_node: focus_iri,
            shape_iri: args.shape_iri.to_owned(),
            path: Some(args.path_iri.to_owned()),
            constraint: "sh:disjoint".to_owned(),
            message: format!(
                "value set for <{}> is not disjoint with <{other_path_iri}>: \
                 shared value ids={shared:?}",
                args.path_iri
            ),
            severity: "Violation".to_owned(),
            sh_value: None,
            sh_source_constraint_component: None,
        });
    }
}

// ─── Numeric range constraints (v0.48.0) ─────────────────────────────────────
//
// sh:minExclusive, sh:maxExclusive, sh:minInclusive, sh:maxInclusive
//
// All four use the `compare_dictionary_values` helper already used by
// sh:lessThan / sh:lessThanOrEquals in shape_based.rs.

use std::cmp::Ordering;

/// Try to look up a SHACL constraint bound value (IRI or literal) as a dictionary ID.
fn lookup_bound_id(value: &str) -> Option<i64> {
    crate::dictionary::lookup_iri(value)
}

/// Numeric value of a bound or decoded term (FORK-SHACL-RANGE-01).
///
/// Accepts the forms the shape parser stores and `dictionary::decode` returns:
/// a bare Turtle number (`20`, `-1.5`, `1e3`), a quoted lexical form with or without a
/// datatype (`"0.0"^^<…#decimal>`, `"7"`). IRIs, language-tagged strings and non-numeric
/// lexical forms return `None`.
pub(crate) fn numeric_value(term: &str) -> Option<f64> {
    let t = term.trim();
    let lexical = if let Some(rest) = t.strip_prefix('"') {
        let end = rest.find('"')?;
        let tail = &rest[end + 1..];
        if tail.starts_with('@') {
            return None; // language-tagged string
        }
        &rest[..end]
    } else if t.starts_with('<') || t.starts_with("_:") {
        return None;
    } else {
        t
    };
    let n = lexical.trim().parse::<f64>().ok()?;
    n.is_finite().then_some(n)
}

/// Shared implementation of the four numeric range constraints.
///
/// 0.136.0 looked the bound up as an IRI in the dictionary, which never matches a literal such as
/// `20` or `"100.0"^^xsd:decimal`, so every range check was silently skipped. Numeric bounds are
/// now compared by value; a value that is not numeric (or not comparable) is a violation, as in
/// SHACL Core §4.3 ("if the comparison cannot be performed, a violation is reported"). Non-numeric
/// bounds (dates, strings) keep the dictionary comparison.
fn check_range(
    bound: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
    constraint: &str,
    op: &str,
    ok: fn(Ordering) -> bool,
) {
    let numeric_bound = numeric_value(bound);
    let bound_id = if numeric_bound.is_none() {
        match lookup_bound_id(bound) {
            Some(id) => Some(id),
            None => return, // non-numeric bound not in dictionary → skip (open world)
        }
    } else {
        None
    };
    for v_id in get_value_ids(args.focus, args.path_id, args.graph_id) {
        let decoded = crate::dictionary::decode(v_id).unwrap_or_default();
        let passed = match (numeric_bound, bound_id) {
            (Some(b), _) => numeric_value(&decoded)
                .and_then(|v| v.partial_cmp(&b))
                .map(ok)
                .unwrap_or(false),
            (None, Some(bid)) => compare_dictionary_values(v_id, bid).map(ok).unwrap_or(true),
            (None, None) => true,
        };
        if !passed {
            violations.push(Violation {
                focus_node: crate::shacl::decode_id_safe(args.focus),
                shape_iri: args.shape_iri.to_owned(),
                path: Some(args.path_iri.to_owned()),
                constraint: constraint.to_owned(),
                message: format!("value '{decoded}' is not {op} {bound}"),
                severity: "Violation".to_owned(),
                sh_value: None,
                sh_source_constraint_component: None,
            });
        }
    }
}

/// Check `sh:minExclusive bound` — every value must be strictly greater than `bound`.
pub(crate) fn check_min_exclusive(
    bound: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
) {
    check_range(bound, args, violations, "sh:minExclusive", ">", |o| {
        o == Ordering::Greater
    });
}

/// Check `sh:maxExclusive bound` — every value must be strictly less than `bound`.
pub(crate) fn check_max_exclusive(
    bound: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
) {
    check_range(bound, args, violations, "sh:maxExclusive", "<", |o| {
        o == Ordering::Less
    });
}

/// Check `sh:minInclusive bound` — every value must be >= `bound`.
pub(crate) fn check_min_inclusive(
    bound: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
) {
    check_range(bound, args, violations, "sh:minInclusive", ">=", |o| {
        o != Ordering::Less
    });
}

/// Check `sh:maxInclusive bound` — every value must be <= `bound`.
pub(crate) fn check_max_inclusive(
    bound: &str,
    args: &ConstraintArgs,
    violations: &mut Vec<Violation>,
) {
    check_range(bound, args, violations, "sh:maxInclusive", "<=", |o| {
        o != Ordering::Greater
    });
}

#[cfg(test)]
mod range_tests {
    use super::numeric_value;

    #[test]
    fn parses_bare_and_typed_numbers() {
        assert_eq!(numeric_value("20"), Some(20.0));
        assert_eq!(numeric_value("-1.5"), Some(-1.5));
        assert_eq!(numeric_value("1e3"), Some(1000.0));
        assert_eq!(
            numeric_value("\"100.0\"^^<http://www.w3.org/2001/XMLSchema#decimal>"),
            Some(100.0)
        );
        assert_eq!(
            numeric_value("\"99\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            Some(99.0)
        );
        assert_eq!(numeric_value("\"7\""), Some(7.0));
    }

    #[test]
    fn rejects_non_numbers() {
        assert_eq!(numeric_value("<http://ex/a>"), None);
        assert_eq!(
            numeric_value("\"abc\"^^<http://www.w3.org/2001/XMLSchema#string>"),
            None
        );
        assert_eq!(numeric_value("\"5\"@en"), None);
        assert_eq!(numeric_value("_:b0"), None);
        assert_eq!(numeric_value("NaN"), None);
        assert_eq!(
            numeric_value("\"2026-10-09\"^^<http://www.w3.org/2001/XMLSchema#date>"),
            None
        );
    }
}
