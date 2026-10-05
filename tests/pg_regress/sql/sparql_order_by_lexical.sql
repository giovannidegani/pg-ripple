-- pg_regress test: SPARQL ORDER BY lexical / value ordering (FORK-ORDERBY-01)
--
-- Before this fix ORDER BY sorted by the BIGINT dictionary ID, so strings and
-- IRIs came back in load order and DESC merely reversed load order.

CREATE EXTENSION IF NOT EXISTS pg_ripple;
SELECT pg_ripple.triple_count() >= 0 AS library_loaded;
SET search_path TO pg_ripple, public;

-- Labels and IRIs are deliberately loaded in non-alphabetical order.
SELECT pg_ripple.load_ntriples(
    '<https://ordlex.test/c/tenant> <https://ordlex.test/label> "Tenant" .' || E'\n' ||
    '<https://ordlex.test/c/user> <https://ordlex.test/label> "User" .' || E'\n' ||
    '<https://ordlex.test/c/profile> <https://ordlex.test/label> "Profile" .' || E'\n' ||
    '<https://ordlex.test/c/goal> <https://ordlex.test/label> "Goal" .' || E'\n' ||
    '<https://ordlex.test/c/program> <https://ordlex.test/label> "Program" .' || E'\n' ||
    '<https://ordlex.test/c/planned> <https://ordlex.test/label> "Planned session" .' || E'\n' ||
    '<https://ordlex.test/c/aardvark> <https://ordlex.test/label> "aardvark"@en .'
) = 7 AS labels_loaded;

SELECT pg_ripple.load_ntriples(
    '<https://ordlex.test/n/a> <https://ordlex.test/val> "10"^^<http://www.w3.org/2001/XMLSchema#integer> .' || E'\n' ||
    '<https://ordlex.test/n/b> <https://ordlex.test/val> "9.5"^^<http://www.w3.org/2001/XMLSchema#decimal> .' || E'\n' ||
    '<https://ordlex.test/n/c> <https://ordlex.test/val> "-1"^^<http://www.w3.org/2001/XMLSchema#integer> .' || E'\n' ||
    '<https://ordlex.test/n/d> <https://ordlex.test/val> "1.5E1"^^<http://www.w3.org/2001/XMLSchema#double> .' || E'\n' ||
    '<https://ordlex.test/n/e> <https://ordlex.test/val> "2"^^<http://www.w3.org/2001/XMLSchema#integer> .'
) = 5 AS numbers_loaded;

SELECT pg_ripple.load_ntriples(
    '<https://ordlex.test/d/a> <https://ordlex.test/when> "2024-03-01T12:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .' || E'\n' ||
    '<https://ordlex.test/d/b> <https://ordlex.test/when> "2024-03-01T09:00:00-05:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> .' || E'\n' ||
    '<https://ordlex.test/d/c> <https://ordlex.test/when> "2023-12-31T23:59:59Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .' || E'\n' ||
    '<https://ordlex.test/d/d> <https://ordlex.test/when> "2024-01-01T00:00:00+02:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> .'
) = 4 AS dates_loaded;

-- 1. Repro from coach-kg docs/SPIKE.md: ORDER BY ?l is alphabetical
--    (codepoint order: upper-case before lower-case).
SELECT string_agg(r.result->>'l', ' | ' ORDER BY r.n) AS asc_labels
FROM pg_ripple.sparql($$
    SELECT ?l WHERE { ?c <https://ordlex.test/label> ?l } ORDER BY ?l
$$) WITH ORDINALITY AS r(result, n);

-- 2. DESC is the true reverse alphabetical order, not reversed load order.
SELECT string_agg(r.result->>'l', ' | ' ORDER BY r.n) AS desc_labels
FROM pg_ripple.sparql($$
    SELECT ?l WHERE { ?c <https://ordlex.test/label> ?l } ORDER BY DESC(?l)
$$) WITH ORDINALITY AS r(result, n);

-- 3. ORDER BY + LIMIT (TopN) returns the alphabetical prefix.
SELECT string_agg(r.result->>'l', ' | ' ORDER BY r.n) AS top3_labels
FROM pg_ripple.sparql($$
    SELECT ?l WHERE { ?c <https://ordlex.test/label> ?l } ORDER BY ?l LIMIT 3
$$) WITH ORDINALITY AS r(result, n);

-- 4. ORDER BY STR(?l) (expression form).
SELECT string_agg(r.result->>'l', ' | ' ORDER BY r.n) AS str_labels
FROM pg_ripple.sparql($$
    SELECT ?l WHERE { ?c <https://ordlex.test/label> ?l } ORDER BY STR(?l)
$$) WITH ORDINALITY AS r(result, n);

-- 5. Case-insensitive ordering via LCASE.
SELECT string_agg(r.result->>'l', ' | ' ORDER BY r.n) AS lcase_labels
FROM pg_ripple.sparql($$
    SELECT ?l WHERE { ?c <https://ordlex.test/label> ?l } ORDER BY LCASE(?l)
$$) WITH ORDINALITY AS r(result, n);

-- 6. IRIs sort by IRI string.
SELECT string_agg(r.result->>'c', ' | ' ORDER BY r.n) AS iris
FROM pg_ripple.sparql($$
    SELECT ?c WHERE { ?c <https://ordlex.test/label> ?l } ORDER BY ?c
$$) WITH ORDINALITY AS r(result, n);

-- 7. SELECT DISTINCT + ORDER BY + LIMIT/OFFSET.
SELECT string_agg(r.result->>'l', ' | ' ORDER BY r.n) AS distinct_labels
FROM pg_ripple.sparql($$
    SELECT DISTINCT ?l WHERE { ?c <https://ordlex.test/label> ?l }
    ORDER BY DESC(?l) LIMIT 3 OFFSET 1
$$) WITH ORDINALITY AS r(result, n);

-- 8. Numeric literals (inline integers + dictionary decimal/double) by value.
SELECT string_agg(r.result->>'v', ' | ' ORDER BY r.n) AS numbers_asc
FROM pg_ripple.sparql($$
    SELECT ?v WHERE { ?s <https://ordlex.test/val> ?v } ORDER BY ?v
$$) WITH ORDINALITY AS r(result, n);

SELECT string_agg(r.result->>'v', ' | ' ORDER BY r.n) AS numbers_desc
FROM pg_ripple.sparql($$
    SELECT ?v WHERE { ?s <https://ordlex.test/val> ?v } ORDER BY DESC(?v)
$$) WITH ORDINALITY AS r(result, n);

-- 9. dateTime values chronologically (inline UTC + dictionary with offset):
--    d/d (2023-12-31T22:00Z) < d/c < d/a (12:00Z) < d/b (14:00Z).
SELECT string_agg(r.result->>'s', ' | ' ORDER BY r.n) AS dates_asc
FROM pg_ripple.sparql($$
    SELECT ?s WHERE { ?s <https://ordlex.test/when> ?w } ORDER BY ?w
$$) WITH ORDINALITY AS r(result, n);

-- 10. Aggregate (raw numeric) ORDER BY still works, with a lexical tie-breaker.
SELECT string_agg((r.result->>'p') || '=' || (r.result->>'cnt'), ' | ' ORDER BY r.n) AS agg_order
FROM pg_ripple.sparql($$
    SELECT ?p (COUNT(?s) AS ?cnt) WHERE {
        ?s ?p ?o .
        FILTER(?p IN (<https://ordlex.test/label>, <https://ordlex.test/val>, <https://ordlex.test/when>))
    }
    GROUP BY ?p ORDER BY DESC(?cnt) ?p
$$) WITH ORDINALITY AS r(result, n);

-- 11. Mixed term kinds: IRIs before literals (SPARQL 1.1 §15.1).
SELECT string_agg(r.result->>'o', ' | ' ORDER BY r.n) AS mixed_kinds
FROM pg_ripple.sparql($$
    SELECT ?o WHERE {
        { <https://ordlex.test/c/goal> <https://ordlex.test/label> ?o }
        UNION { BIND(<https://ordlex.test/z> AS ?o) }
        UNION { <https://ordlex.test/n/e> <https://ordlex.test/val> ?o }
    } ORDER BY ?o
$$) WITH ORDINALITY AS r(result, n);

-- 12. Computed ORDER BY expression: terms minted during the statement by
--     encode_typed_literal() are not visible to the statement snapshot and are
--     resolved through the SPI decode fallback.  DESC(?v * -1) == ASC(?v).
SELECT string_agg(r.result->>'v', ' | ' ORDER BY r.n) AS computed_expr
FROM pg_ripple.sparql($$
    SELECT ?v WHERE { ?s <https://ordlex.test/val> ?v } ORDER BY DESC(?v * -1)
$$) WITH ORDINALITY AS r(result, n);

-- 13. ORDER BY a BIND(...) variable holding a computed term (the computed
--     source is evaluated once in a fenced inner query).  ?w = -2 * ?v, so
--     ascending ?w is descending ?v.
SELECT string_agg(r.result->>'s', ' | ' ORDER BY r.n) AS bind_computed
FROM pg_ripple.sparql($$
    SELECT ?s WHERE {
        ?s <https://ordlex.test/val> ?v
        BIND(?v * -2 AS ?w)
    } ORDER BY ?w
$$) WITH ORDINALITY AS r(result, n);
