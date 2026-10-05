//! SPARQL filter pattern dispatch utilities — SQL identifier sanitizer,
//! modifier extraction, ORDER BY translator, VALUES handler, and BIND (Extend).
//!
//! See [`filter_expr`](super::filter_expr) for the expression compilation half.

use std::collections::HashMap;

use spargebra::algebra::{Expression, GraphPattern, OrderExpression};
use spargebra::term::{GroundTerm, Literal};

use crate::dictionary;
use crate::sparql::sqlgen::{Ctx, Fragment};
use crate::sparql::translate::group::quote_sql_string;

// ─── SQL identifier sanitizer ─────────────────────────────────────────────────

/// Sanitize a SPARQL variable name for use as a SQL column alias.
pub(crate) fn sanitize_sql_ident(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ─── Modifier extraction helpers ─────────────────────────────────────────────

pub(crate) struct Modifiers<'a> {
    pub(crate) pattern: &'a GraphPattern,
    pub(crate) project_vars: Option<Vec<String>>,
    pub(crate) distinct: bool,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: usize,
    pub(crate) order_exprs: Vec<OrderExpression>,
}

pub(crate) fn extract_modifiers(mut p: &GraphPattern) -> Modifiers<'_> {
    let mut project_vars: Option<Vec<String>> = None;
    let mut distinct = false;
    let mut limit: Option<usize> = None;
    let mut offset = 0usize;
    let mut order_exprs: Vec<OrderExpression> = vec![];

    loop {
        match p {
            GraphPattern::Project { inner, variables } => {
                if project_vars.is_none() {
                    project_vars = Some(variables.iter().map(|v| v.as_str().to_owned()).collect());
                }
                p = inner;
            }
            GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
                distinct = true;
                p = inner;
            }
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => {
                offset = *start;
                limit = *length;
                p = inner;
            }
            GraphPattern::OrderBy { inner, expression } => {
                order_exprs = expression.clone();
                p = inner;
            }
            _ => break,
        }
    }

    Modifiers {
        pattern: p,
        project_vars,
        distinct,
        limit,
        offset,
        order_exprs,
    }
}

// ─── ORDER BY translator ──────────────────────────────────────────────────────
//
// FORK-ORDERBY-01: SPARQL ORDER BY used to emit `ORDER BY <id column>`, i.e. it
// sorted by the BIGINT dictionary ID.  Dictionary IDs are IDENTITY values, so
// strings and IRIs came back in *load order* and `DESC` merely reversed it.
// Only inline-encoded integers/dates happened to sort correctly (their bit
// packing is order-preserving).
//
// The translator now emits a composite sort key per ORDER BY condition that
// follows SPARQL 1.1 §15.1 for the term categories:
//
//   blank node < IRI < numeric < boolean < date/dateTime < other literals
//                                                         < quoted triples
//
// * numeric literals (inline xsd:integer and dictionary-resident
//   xsd:decimal/double/float/derived integer types) compare by value,
// * xsd:date / xsd:dateTime compare chronologically,
// * IRIs, blank nodes, plain / lang-tagged / xsd:string and other typed
//   literals compare by their lexical form using codepoint (`COLLATE "C"`)
//   order, matching `fn:compare` with the codepoint collation,
// * the dictionary ID is the final tie-breaker so the order is deterministic.
//
// Unbound values keep the previous behaviour (NULLS LAST for ASC, NULLS FIRST
// for DESC).  Raw SQL values (aggregate outputs, STRLEN, GROUP_CONCAT …) are
// ordered directly by their SQL value.

const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";

/// XSD numeric datatypes whose lexical form is ordered by numeric value.
const XSD_NUMERIC_LOCAL: &[&str] = &[
    "integer",
    "decimal",
    "float",
    "double",
    "long",
    "int",
    "short",
    "byte",
    "nonNegativeInteger",
    "nonPositiveInteger",
    "negativeInteger",
    "positiveInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
    "unsignedByte",
];

fn xsd_numeric_in_list() -> String {
    XSD_NUMERIC_LOCAL
        .iter()
        .map(|l| format!("'{XSD_NS}{l}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Bits 62–56 of an inline-encoded ID (see `dictionary::inline`).
fn inline_type_sql(col: &str) -> String {
    format!("((({col}) >> 56) & 127)")
}

/// Signed 56-bit payload of an inline-encoded ID (offset by 2^55 removed).
fn inline_value_sql(col: &str) -> String {
    format!("((({col}) & 72057594037927935::bigint) - 36028797018963968::bigint)")
}

/// Wrap a per-term CASE expression in a lookup of the dictionary row for `col`.
///
/// The `LEFT JOIN` always yields exactly one row, so inside `body` the alias
/// `_ok` is NULL (`_ok.id IS NULL`) when the ID is not visible to the
/// statement snapshot.  That happens for terms minted *during* the statement
/// by `encode_term()` / `encode_typed_literal()` (BIND / ORDER BY expressions
/// such as `?v * -1` or `CONCAT(…)`): those rows are only reachable through
/// the SPI-backed `pg_ripple.decode_id()` / `pg_ripple.decode_numeric_spi()`.
///
/// With `joined = Some(alias)` the caller has already `LEFT JOIN`ed
/// `_pg_ripple.dictionary {alias} ON {alias}.id = col` (see
/// `build_ordered_select`) and `body` is used as-is.
fn dict_lookup_sql(col: &str, body: &str, joined: Option<&str>) -> String {
    match joined {
        Some(_) => format!("({body})"),
        None => format!(
            "(SELECT {body} FROM (SELECT 1) _one \
              LEFT JOIN _pg_ripple.dictionary _ok ON _ok.id = ({col}))"
        ),
    }
}

/// N-Triples form of `col` via SPI (sees rows inserted earlier in the statement).
fn spi_ntriples_sql(col: &str) -> String {
    format!("pg_ripple.decode_id({col})")
}

/// Lexical form extracted from the SPI N-Triples rendering of `col`.
fn spi_lexical_sql(col: &str) -> String {
    let nt = spi_ntriples_sql(col);
    format!(
        "(CASE WHEN left({nt}, 1) = '\"' \
               THEN regexp_replace({nt}, '^\"(.*)\"(\\^\\^<[^>]*>|@[A-Za-z0-9-]+)?$', '\\1', 's') \
               WHEN left({nt}, 1) = '<' AND left({nt}, 2) <> '<<' \
               THEN substr({nt}, 2, length({nt}) - 2) \
               ELSE {nt} END)"
    )
}

/// Category rank of an RDF term held as a dictionary ID (SPARQL 1.1 §15.1).
pub(crate) fn term_sort_rank_sql(col: &str, joined: Option<&str>) -> String {
    let d = joined.unwrap_or("_ok");
    let it = inline_type_sql(col);
    let nums = xsd_numeric_in_list();
    let nt = spi_ntriples_sql(col);
    let body = format!(
        "CASE WHEN {d}.id IS NULL THEN (CASE \
                  WHEN left({nt}, 2) = '_:' THEN 0 \
                  WHEN left({nt}, 2) = '<<' THEN 7 \
                  WHEN left({nt}, 1) = '<' THEN 1 \
                  WHEN pg_ripple.decode_numeric_spi({col}) IS NOT NULL THEN 3 \
                  WHEN {nt} ~ '\\^\\^<{XSD_NS}boolean>$' THEN 4 \
                  WHEN {nt} ~ '\\^\\^<{XSD_NS}(date|dateTime|dateTimeStamp)>$' THEN 5 \
                  ELSE 6 END) \
              ELSE (CASE {d}.kind \
                  WHEN 1 THEN 0 \
                  WHEN 0 THEN 1 \
                  WHEN 3 THEN (CASE \
                      WHEN {d}.datatype IN ({nums}) THEN 3 \
                      WHEN {d}.datatype = '{XSD_NS}boolean' THEN 4 \
                      WHEN {d}.datatype IN ('{XSD_NS}date', '{XSD_NS}dateTime', \
                                            '{XSD_NS}dateTimeStamp') THEN 5 \
                      ELSE 6 END) \
                  WHEN 5 THEN 7 \
                  ELSE 6 END) \
         END"
    );
    let lookup = dict_lookup_sql(col, &body, joined);
    format!(
        "CASE WHEN ({col}) IS NULL THEN NULL \
              WHEN ({col}) < 0 THEN (CASE {it} WHEN 0 THEN 3 WHEN 1 THEN 4 \
                                     WHEN 2 THEN 5 WHEN 3 THEN 5 ELSE 6 END) \
              ELSE {lookup} \
         END"
    )
}

/// SQL numeric value of a lexical form (`lex` is a SQL text expression),
/// NULL when it is not a valid numeric lexical form.
fn numeric_from_lexical_sql(lex: &str) -> String {
    format!(
        "(CASE WHEN btrim({lex}) IN ('INF', '+INF') THEN 'Infinity'::numeric \
               WHEN btrim({lex}) = '-INF' THEN '-Infinity'::numeric \
               WHEN pg_input_is_valid(btrim({lex}), 'numeric') THEN btrim({lex})::numeric \
               ELSE NULL END)"
    )
}

/// Numeric sort key (numeric / boolean literals), NULL for everything else.
pub(crate) fn term_sort_numeric_sql(col: &str, joined: Option<&str>) -> String {
    let d = joined.unwrap_or("_ok");
    let it = inline_type_sql(col);
    let iv = inline_value_sql(col);
    let nums = xsd_numeric_in_list();
    let dict_num = numeric_from_lexical_sql(&format!("{d}.value"));
    let body = format!(
        "CASE WHEN {d}.id IS NULL THEN pg_ripple.decode_numeric_spi({col}) \
              WHEN {d}.kind = 3 AND {d}.datatype IN ({nums}) THEN {dict_num} \
              WHEN {d}.kind = 3 AND {d}.datatype = '{XSD_NS}boolean' THEN \
                  (CASE WHEN btrim({d}.value) IN ('true', '1') THEN 1 ELSE 0 END)::numeric \
              ELSE NULL END"
    );
    let lookup = dict_lookup_sql(col, &body, joined);
    format!(
        "CASE WHEN ({col}) < 0 THEN (CASE {it} WHEN 0 THEN ({iv})::numeric \
                                     WHEN 1 THEN ((({col}) & 1))::numeric \
                                     ELSE NULL END) \
              WHEN ({col}) > 0 THEN {lookup} \
              ELSE NULL END"
    )
}

/// Chronological value of an xsd:dateTime / xsd:date lexical form.
fn time_from_lexical_sql(lex: &str, is_datetime: &str, is_date: &str) -> String {
    format!(
        "(CASE WHEN ({is_datetime}) AND {lex} ~ '^[0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}}T' \
                    AND pg_input_is_valid({lex}, 'timestamptz') THEN ({lex})::timestamptz \
               WHEN ({is_date}) AND {lex} ~ '^[0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}}' \
                    AND pg_input_is_valid(left({lex}, 10), 'date') \
                    THEN left({lex}, 10)::date::timestamptz \
               ELSE NULL END)"
    )
}

/// Chronological sort key (xsd:date / xsd:dateTime), NULL for everything else.
pub(crate) fn term_sort_time_sql(col: &str, joined: Option<&str>) -> String {
    let d = joined.unwrap_or("_ok");
    let it = inline_type_sql(col);
    let iv = inline_value_sql(col);
    let nt = spi_ntriples_sql(col);
    let dict_time = time_from_lexical_sql(
        &format!("{d}.value"),
        &format!("{d}.kind = 3 AND {d}.datatype IN ('{XSD_NS}dateTime', '{XSD_NS}dateTimeStamp')"),
        &format!("{d}.kind = 3 AND {d}.datatype = '{XSD_NS}date'"),
    );
    let spi_time = time_from_lexical_sql(
        &spi_lexical_sql(col),
        &format!("{nt} ~ '\\^\\^<{XSD_NS}(dateTime|dateTimeStamp)>$'"),
        &format!("{nt} ~ '\\^\\^<{XSD_NS}date>$'"),
    );
    let body = format!("CASE WHEN {d}.id IS NULL THEN {spi_time} ELSE {dict_time} END");
    let lookup = dict_lookup_sql(col, &body, joined);
    format!(
        "CASE WHEN ({col}) < 0 THEN (CASE {it} \
                  WHEN 2 THEN timestamptz 'epoch' + ({iv}) * interval '1 microsecond' \
                  WHEN 3 THEN (CASE WHEN ({iv}) BETWEEN -2440000 AND 2000000000 \
                                    THEN (date '1970-01-01' + ({iv})::int)::timestamptz \
                                    ELSE NULL END) \
                  ELSE NULL END) \
              WHEN ({col}) > 0 THEN {lookup} \
              ELSE NULL END"
    )
}

/// Lexical sort key (IRI string / literal lexical form) in codepoint order.
/// Inline-encoded values carry no lexical key: they are fully ordered by the
/// numeric / chronological keys.
pub(crate) fn term_sort_text_sql(col: &str, joined: Option<&str>) -> String {
    let d = joined.unwrap_or("_ok");
    let spi_lex = spi_lexical_sql(col);
    let body = format!("CASE WHEN {d}.id IS NULL THEN {spi_lex} ELSE {d}.value END");
    let lookup = dict_lookup_sql(col, &body, joined);
    format!("(CASE WHEN ({col}) > 0 THEN {lookup} ELSE NULL END) COLLATE \"C\"")
}

/// Ordered list of SQL sort keys for a dictionary-ID-valued expression.
///
/// `joined` names a `LEFT JOIN`ed dictionary alias for `col` (one join instead
/// of four correlated lookups); `None` uses correlated subqueries.
pub(crate) fn term_sort_keys(col: &str, joined: Option<&str>) -> Vec<String> {
    vec![
        term_sort_rank_sql(col, joined),
        term_sort_numeric_sql(col, joined),
        term_sort_time_sql(col, joined),
        term_sort_text_sql(col, joined),
        format!("({col})"),
    ]
}

/// SQL text expression for the lexical form of a dictionary-ID expression
/// (works for inline and dictionary-resident IDs).
fn lexical_text_sql(col: &str) -> String {
    format!(
        "({})",
        crate::sparql::expr::decode_lexical_sql(&format!("({col})"))
    )
}

/// Translate string-valued ORDER BY expressions (`STR`, `LCASE`, `UCASE`)
/// directly to a SQL text expression, avoiding dictionary writes from
/// `encode_term()` in value context.
fn order_text_expr(
    expr: &Expression,
    bindings: &HashMap<String, String>,
    ctx: &mut Ctx,
) -> Option<String> {
    use spargebra::algebra::Function;
    match expr {
        Expression::Variable(v) => {
            let col = bindings.get(v.as_str())?;
            let name = v.as_str();
            if ctx.raw_text_vars.contains(name)
                || ctx.raw_iri_vars.contains(name)
                || ctx.raw_numeric_vars.contains(name)
                || ctx.raw_double_vars.contains(name)
            {
                Some(format!("({col})::text"))
            } else {
                Some(lexical_text_sql(col))
            }
        }
        Expression::Literal(lit) => Some(quote_sql_string(lit.value())),
        Expression::NamedNode(nn) => Some(quote_sql_string(nn.as_str())),
        Expression::FunctionCall(Function::Str, args) if args.len() == 1 => {
            order_text_expr(&args[0], bindings, ctx)
        }
        Expression::FunctionCall(Function::LCase, args) if args.len() == 1 => Some(format!(
            "lower({})",
            order_text_expr(&args[0], bindings, ctx)?
        )),
        Expression::FunctionCall(Function::UCase, args) if args.len() == 1 => Some(format!(
            "upper({})",
            order_text_expr(&args[0], bindings, ctx)?
        )),
        _ => None,
    }
}

/// Where the value of one ORDER BY condition comes from.
#[derive(Clone, Debug)]
pub(crate) enum OrderKeySource {
    /// SQL expression yielding a dictionary ID → full term sort keys.
    Term(String),
    /// Like `Term`, with the dictionary row already `LEFT JOIN`ed as `dict`.
    TermJoined { col: String, dict: String },
    /// Raw SQL value (aggregate output, numeric function) ordered as-is.
    Raw(String),
    /// SQL text value ordered in codepoint order.
    Text(String),
}

/// One translated ORDER BY condition.
#[derive(Clone, Debug)]
pub(crate) struct OrderKey {
    pub(crate) source: OrderKeySource,
    pub(crate) desc: bool,
}

/// Translate one ORDER BY condition, or `None` when the expression cannot be
/// translated (the condition is then dropped, as before).
fn order_key_source(
    expr: &Expression,
    bindings: &HashMap<String, String>,
    ctx: &mut Ctx,
) -> Option<OrderKeySource> {
    use spargebra::algebra::Function;
    match expr {
        Expression::Variable(v) => {
            let col = bindings.get(v.as_str())?;
            let name = v.as_str();
            if ctx.raw_numeric_vars.contains(name) || ctx.raw_double_vars.contains(name) {
                Some(OrderKeySource::Raw(col.clone()))
            } else if ctx.raw_text_vars.contains(name) || ctx.raw_iri_vars.contains(name) {
                Some(OrderKeySource::Text(format!("({col})::text")))
            } else {
                Some(OrderKeySource::Term(col.clone()))
            }
        }
        Expression::FunctionCall(Function::Str | Function::LCase | Function::UCase, _) => {
            order_text_expr(expr, bindings, ctx).map(OrderKeySource::Text)
        }
        _ if super::filter_expr::expr_is_raw_numeric(expr, ctx) => {
            super::filter_expr::translate_expr_value_raw(expr, bindings, ctx)
                .map(OrderKeySource::Raw)
        }
        _ => {
            super::filter_expr::translate_expr_value(expr, bindings, ctx).map(OrderKeySource::Term)
        }
    }
}

/// Translate SPARQL ORDER BY conditions into structured sort keys.
pub(crate) fn order_keys(
    exprs: &[OrderExpression],
    bindings: &HashMap<String, String>,
    ctx: &mut Ctx,
) -> Vec<OrderKey> {
    exprs
        .iter()
        .filter_map(|oe| {
            let (expr, desc) = match oe {
                OrderExpression::Asc(e) => (e, false),
                OrderExpression::Desc(e) => (e, true),
            };
            order_key_source(expr, bindings, ctx).map(|source| OrderKey { source, desc })
        })
        .collect()
}

/// Render sort keys as a SQL `ORDER BY` key list (without the keyword).
pub(crate) fn render_order_keys(keys: &[OrderKey]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for k in keys {
        let dir = if k.desc {
            "DESC NULLS FIRST"
        } else {
            "ASC NULLS LAST"
        };
        match &k.source {
            OrderKeySource::Term(col) => {
                for key in term_sort_keys(col, None) {
                    parts.push(format!("{key} {dir}"));
                }
            }
            OrderKeySource::TermJoined { col, dict } => {
                for key in term_sort_keys(col, Some(dict)) {
                    parts.push(format!("{key} {dir}"));
                }
            }
            OrderKeySource::Raw(sql) => parts.push(format!("({sql}) {dir}")),
            OrderKeySource::Text(sql) => parts.push(format!("({sql}) COLLATE \"C\" {dir}")),
        }
    }
    parts.join(", ")
}

/// Build a SELECT statement with SPARQL ORDER BY semantics.
///
/// * `select_list` — the projection, producing exactly the columns `out_cols`;
/// * `var_cols` — `(sparql variable, output column)` pairs for projected
///   variables (used to resolve ORDER BY over DISTINCT results);
/// * `from_where` — `FROM … WHERE …` text;
/// * `tail` — `LIMIT … OFFSET …` text.
///
/// Term sort keys repeat their source expression several times, so whenever
/// a term key is present every ORDER BY source is evaluated once in an inner
/// query fenced with `OFFSET 0` and the outer query orders by the resulting
/// columns (computed BIND / arithmetic / CONCAT sources would otherwise
/// multiply planning time and `encode_term()` calls), with one `LEFT JOIN`
/// of the dictionary per term key.  SELECT DISTINCT results are always
/// ordered in an outer query because PostgreSQL requires DISTINCT ORDER BY
/// expressions to appear in the select list.
///
/// Returns the SQL and whether an ORDER BY clause was emitted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_ordered_select(
    distinct: bool,
    select_list: &str,
    out_cols: &[String],
    var_cols: &[(String, String)],
    from_where: &str,
    order_exprs: &[OrderExpression],
    bindings: &HashMap<String, String>,
    ctx: &mut Ctx,
    tail: &str,
) -> (String, bool) {
    let distinct_kw = if distinct { "DISTINCT " } else { "" };
    let plain = || format!("SELECT {distinct_kw}{select_list} {from_where} {tail}");
    if order_exprs.is_empty() {
        return (plain(), false);
    }

    // Resolve the ORDER BY sources.  Over DISTINCT results only projected
    // variables are reachable (SPARQL 1.1 §15: ordering by non-projected
    // variables is implementation-defined; such conditions are dropped).
    let (keys, inner_from, inner_star) = if distinct {
        let ob: HashMap<String, String> = var_cols
            .iter()
            .map(|(v, c)| (v.clone(), format!("_d.{c}")))
            .collect();
        let keys = order_keys(order_exprs, &ob, ctx);
        (
            keys,
            format!("FROM (SELECT DISTINCT {select_list} {from_where}) _d"),
            "_d.*".to_owned(),
        )
    } else {
        let keys = order_keys(order_exprs, bindings, ctx);
        (keys, from_where.to_owned(), select_list.to_owned())
    };
    if keys.is_empty() {
        return (plain(), false);
    }

    // Term keys need dictionary lookups: hoist every key source into a fenced
    // inner query (computed sources are evaluated once) and LEFT JOIN the
    // dictionary once per term key instead of four correlated lookups.
    let has_term = keys
        .iter()
        .any(|k| matches!(&k.source, OrderKeySource::Term(_)));

    if !has_term {
        let ob = render_order_keys(&keys);
        let sql = format!("SELECT {inner_star} {inner_from} ORDER BY {ob} {tail}");
        return (sql, true);
    }

    let mut hoisted: Vec<String> = Vec::with_capacity(keys.len());
    let mut joins: Vec<String> = Vec::new();
    let mut outer_keys: Vec<OrderKey> = Vec::with_capacity(keys.len());
    for (i, k) in keys.iter().enumerate() {
        let name = format!("_ok{i}");
        let col = format!("_o.{name}");
        let (sql, src) = match &k.source {
            OrderKeySource::Term(s) | OrderKeySource::TermJoined { col: s, .. } => {
                let dict = format!("_od{i}");
                joins.push(format!(
                    "LEFT JOIN _pg_ripple.dictionary {dict} ON {dict}.id = {col}"
                ));
                (s, OrderKeySource::TermJoined { col, dict })
            }
            OrderKeySource::Raw(s) => (s, OrderKeySource::Raw(col)),
            OrderKeySource::Text(s) => (s, OrderKeySource::Text(col)),
        };
        hoisted.push(format!("({sql}) AS {name}"));
        outer_keys.push(OrderKey {
            source: src,
            desc: k.desc,
        });
    }
    let outer_cols: Vec<String> = out_cols.iter().map(|c| format!("_o.{c}")).collect();
    let ob = render_order_keys(&outer_keys);
    let sql = format!(
        "SELECT {} FROM (SELECT {inner_star}, {} {inner_from} OFFSET 0) _o {} \
         ORDER BY {ob} {tail}",
        outer_cols.join(", "),
        hoisted.join(", "),
        joins.join(" ")
    );
    (sql, true)
}

// ─── VALUES translator ────────────────────────────────────────────────────────

pub(crate) fn translate_values(
    variables: &[spargebra::term::Variable],
    bindings: &[Vec<Option<GroundTerm>>],
    ctx: &mut Ctx,
) -> Fragment {
    if variables.is_empty() || bindings.is_empty() {
        let mut frag = Fragment::empty();
        frag.conditions.push("FALSE".to_owned());
        return frag;
    }

    let mut rows: Vec<String> = Vec::with_capacity(bindings.len());
    let mut encode_ctx: Ctx = Ctx::new();

    for row in bindings {
        let cells: Vec<String> = variables
            .iter()
            .zip(row.iter())
            .map(|(_, cell)| match cell {
                None => "NULL::bigint".to_owned(),
                Some(gt) => {
                    let id = encode_ground_term(gt, &mut encode_ctx);
                    if ctx.parameterize_values {
                        let parameter = ctx.parameters.len() + 1;
                        ctx.parameters.push(id);
                        format!("${parameter}::bigint")
                    } else {
                        id.to_string()
                    }
                }
            })
            .collect();
        rows.push(format!("({})", cells.join(", ")));
    }

    let col_names: Vec<String> = variables
        .iter()
        .map(|v| format!("_val_{}", v.as_str()))
        .collect();

    let col_names_str = col_names.join(", ");
    let n = ctx.alias_counter;
    ctx.alias_counter += 1;
    let values_expr = format!(
        "(SELECT * FROM (VALUES {}) AS _vi{n}({col_names_str}))",
        rows.join(", ")
    );

    let alias = ctx.next_alias();
    let mut frag = Fragment::empty();
    frag.from_items.push((alias.clone(), values_expr));

    for v in variables {
        frag.bindings.insert(
            v.as_str().to_owned(),
            format!("{alias}._val_{}", v.as_str()),
        );
    }

    frag
}

pub(crate) fn encode_ground_term(gt: &GroundTerm, ctx: &mut Ctx) -> i64 {
    match gt {
        GroundTerm::NamedNode(nn) => ctx.encode_iri(nn.as_str()).unwrap_or(0),
        GroundTerm::Literal(lit) => ctx.encode_literal(lit),
        GroundTerm::Triple(t) => {
            let s_id = ctx.encode_iri(t.subject.as_str()).unwrap_or(0);
            let p_id = ctx.encode_iri(t.predicate.as_str()).unwrap_or(0);
            let o_id = encode_ground_term(&t.object, ctx);
            dictionary::lookup_quoted_triple(s_id, p_id, o_id).unwrap_or(0)
        }
    }
}

// ─── Literal lexical helpers (used by filter_expr) ────────────────────────────

pub(crate) fn literal_lexical_value(lit: &Literal) -> String {
    let val = lit.value().replace('\'', "''");
    format!("'{val}'")
}
