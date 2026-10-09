-- pg_regress test (FORK-NESTED-OPT): a loaded shape with sh:minCount 1 on a predicate must not
-- turn an OPTIONAL over that predicate into an INNER JOIN. Before the fix the inner OPTIONAL in
-- `<s> :l ?x . OPTIONAL { <s> :p ?e . OPTIONAL { ?e rdfs:label ?el } }` became an INNER JOIN
-- (the shape only targets :Thing, ?e is not one), so the outer ?e came back null.
CREATE EXTENSION IF NOT EXISTS pg_ripple;
SELECT pg_ripple.load_shacl('
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<https://nopt.test/ThingShape> a sh:NodeShape ; sh:targetClass <https://nopt.test/Thing> ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] .') >= 0 AS shape_loaded;
SELECT pg_ripple.sparql_update('INSERT DATA { GRAPH <https://nopt.test/g> {
  <https://nopt.test/s> <https://nopt.test/p> <https://nopt.test/e> .
  <https://nopt.test/s> <https://nopt.test/l> "S" .
  <https://nopt.test/s2> <https://nopt.test/l> "S2" .
  <https://nopt.test/s2> <https://nopt.test/p> <https://nopt.test/e2> .
  <https://nopt.test/e2> <http://www.w3.org/2000/01/rdf-schema#label> "E2" . } }') >= 0 AS inserted;
SHOW pg_ripple.shacl_optional_promotion;
-- nested OPTIONAL: outer binding survives when the inner one has no match
SELECT result->>'e' AS e, result->>'el' AS el FROM pg_ripple.sparql(
  'SELECT ?e ?el WHERE { GRAPH <https://nopt.test/g> { <https://nopt.test/s> <https://nopt.test/l> ?x .
     OPTIONAL { <https://nopt.test/s> <https://nopt.test/p> ?e . OPTIONAL { ?e <http://www.w3.org/2000/01/rdf-schema#label> ?el } } } }') AS result;
-- and binds both when it does
SELECT result->>'e' AS e, result->>'el' AS el FROM pg_ripple.sparql(
  'SELECT ?e ?el WHERE { GRAPH <https://nopt.test/g> { <https://nopt.test/s2> <https://nopt.test/l> ?x .
     OPTIONAL { <https://nopt.test/s2> <https://nopt.test/p> ?e . OPTIONAL { ?e <http://www.w3.org/2000/01/rdf-schema#label> ?el } } } }') AS result;
-- flat OPTIONAL over the minCount predicate keeps unlabelled subjects
SELECT count(*) AS flat_rows FROM pg_ripple.sparql(
  'SELECT ?s ?lab WHERE { GRAPH <https://nopt.test/g> { ?s <https://nopt.test/l> ?x . OPTIONAL { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?lab } } }');
SELECT pg_ripple.explain_sparql(
  'SELECT ?s ?lab WHERE { GRAPH <https://nopt.test/g> { ?s <https://nopt.test/l> ?x . OPTIONAL { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?lab } } }', 'sql')
  ILIKE '%LEFT JOIN%' AS uses_left_join;
-- the old promotion is still available as an explicit opt-in
SET pg_ripple.shacl_optional_promotion = on;
SELECT pg_ripple.explain_sparql(
  'SELECT ?s ?lab WHERE { GRAPH <https://nopt.test/g> { ?s <https://nopt.test/l> ?x . OPTIONAL { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?lab } } }', 'sql')
  ILIKE '%INNER JOIN%' AS opt_in_promotes;
RESET pg_ripple.shacl_optional_promotion;
SELECT pg_ripple.sparql_update('CLEAR GRAPH <https://nopt.test/g>') >= 0 AS cleared;
SELECT pg_ripple.drop_shape('https://nopt.test/ThingShape') >= 0 AS shape_dropped;
