-- pg_regress test: SHACL numeric range constraints are enforced (FORK-SHACL-RANGE-01)
--
-- In 0.136.0 sh:minInclusive / sh:maxInclusive / sh:minExclusive / sh:maxExclusive bounds were
-- looked up as IRIs in the dictionary, never matched a literal, and every range check was skipped.

CREATE EXTENSION IF NOT EXISTS pg_ripple;
SELECT pg_ripple.triple_count() >= 0 AS library_loaded;
SET search_path TO pg_ripple, public;

SELECT pg_ripple.load_shacl($$
  @prefix sh: <http://www.w3.org/ns/shacl#> .
  @prefix ex: <https://range.test/> .
  @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
  ex:SlotShape a sh:NodeShape ;
    sh:targetClass ex:Slot ;
    sh:property [ sh:path ex:sets ; sh:minInclusive 1 ; sh:maxInclusive 20 ] ;
    sh:property [ sh:path ex:loadKg ; sh:minInclusive "0.0"^^xsd:decimal ; sh:maxInclusive "1000.0"^^xsd:decimal ] ;
    sh:property [ sh:path ex:rpe ; sh:minExclusive 0 ; sh:maxExclusive 10.5 ] .
$$) >= 0 AS shacl_loaded;

SELECT pg_ripple.load_turtle_into_graph($$
  @prefix ex: <https://range.test/> .
  ex:ok    a ex:Slot ; ex:sets 20 ; ex:loadKg 1000.0 ; ex:rpe 10.4 .
  ex:edge  a ex:Slot ; ex:sets 1 ; ex:loadKg 0.0 ; ex:rpe 0.5 .
  ex:big   a ex:Slot ; ex:sets 99 ; ex:loadKg 2000.0 ; ex:rpe 10.5 .
  ex:small a ex:Slot ; ex:sets 0 ; ex:loadKg -0.5 ; ex:rpe 0 .
  ex:text  a ex:Slot ; ex:sets "many" .
$$, 'https://range.test/g') > 0 AS data_loaded;

-- Conforming slots (incl. inclusive edges) produce no violations; every out-of-range,
-- exclusive-edge and non-numeric value does.
SELECT v->>'focusNode' AS focus, v->>'path' AS path, v->>'constraint' AS constraint
FROM jsonb_array_elements(pg_ripple.validate('https://range.test/g')->'violations') v
ORDER BY 1, 2, 3;

SELECT (pg_ripple.validate('https://range.test/g')->>'conforms')::boolean AS conforms;

-- Removing the bad slots makes the graph conform.
SELECT pg_ripple.sparql_update($$
  DELETE WHERE { GRAPH <https://range.test/g> { <https://range.test/big> ?p ?o } } ;
  DELETE WHERE { GRAPH <https://range.test/g> { <https://range.test/small> ?p ?o } } ;
  DELETE WHERE { GRAPH <https://range.test/g> { <https://range.test/text> ?p ?o } }
$$) >= 0 AS cleaned;
SELECT (pg_ripple.validate('https://range.test/g')->>'conforms')::boolean AS conforms_after_cleanup;

SELECT pg_ripple.clear_graph('https://range.test/g') >= 0 AS cleared;
SELECT pg_ripple.drop_shape('https://range.test/SlotShape') >= 0 AS shape_dropped;
