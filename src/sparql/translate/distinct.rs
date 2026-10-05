//! Slice (LIMIT/OFFSET with nested subquery) translator.

use spargebra::algebra::GraphPattern;

use crate::sparql::sqlgen::{Ctx, Fragment};
use crate::sparql::translate::filter::{build_ordered_select, extract_modifiers};

/// Translate a Slice node (nested LIMIT/OFFSET subquery).
pub(crate) fn translate_slice(pattern: &GraphPattern, ctx: &mut Ctx) -> Fragment {
    let mods = extract_modifiers(pattern);
    let inner_frag = crate::sparql::sqlgen::translate_pattern(mods.pattern, ctx);

    let keep_vars: Vec<String> = if let Some(ref pv) = mods.project_vars {
        pv.clone()
    } else {
        inner_frag.bindings.keys().cloned().collect()
    };

    let cols: Vec<String> = keep_vars
        .iter()
        .filter_map(|v| {
            inner_frag
                .bindings
                .get(v)
                .map(|col| format!("{col} AS _sl_{v}"))
        })
        .collect();

    let select_clause = if cols.is_empty() {
        "1 AS _sl_dummy".to_owned()
    } else {
        cols.join(", ")
    };
    let out_cols: Vec<String> = if cols.is_empty() {
        vec!["_sl_dummy".to_owned()]
    } else {
        keep_vars
            .iter()
            .filter(|v| inner_frag.bindings.contains_key(*v))
            .map(|v| format!("_sl_{v}"))
            .collect()
    };
    let var_cols: Vec<(String, String)> = keep_vars
        .iter()
        .filter(|v| inner_frag.bindings.contains_key(*v))
        .map(|v| (v.clone(), format!("_sl_{v}")))
        .collect();

    let limit_str = mods.limit.map_or(String::new(), |n| format!("LIMIT {n}"));
    let offset_str = if mods.offset > 0 {
        format!("OFFSET {}", mods.offset)
    } else {
        String::new()
    };

    // FORK-ORDERBY-01: lexical / value ORDER BY (see `build_ordered_select`).
    // DISTINCT is not applied here (unchanged behaviour of this translator).
    let (inner_sql, _ordered) = build_ordered_select(
        false,
        &select_clause,
        &out_cols,
        &var_cols,
        &format!(
            "FROM {} {}",
            inner_frag.build_from(),
            inner_frag.build_where()
        ),
        &mods.order_exprs,
        &inner_frag.bindings,
        ctx,
        &format!("{limit_str} {offset_str}"),
    );
    let subq = format!("({inner_sql})");

    let alias = ctx.next_alias();
    let mut frag = Fragment::empty();
    frag.from_items.push((alias.clone(), subq));
    for v in &keep_vars {
        if inner_frag.bindings.contains_key(v) {
            frag.bindings.insert(v.clone(), format!("{alias}._sl_{v}"));
        }
    }
    frag
}
