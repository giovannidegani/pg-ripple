-- pg_regress test: SPARQL SUM/AVG + numeric_type_code_spi (FORK-SUM-01)
--
-- SUM/AVG emit SQL that calls pg_ripple.numeric_type_code_spi(bigint). That
-- helper was added after the v0.136.0 tag without a version bump, so catalogs
-- created from the released image lacked it and SUM failed with
-- "function pg_ripple.numeric_type_code_spi(bigint) does not exist".

CREATE EXTENSION IF NOT EXISTS pg_ripple;
SELECT pg_ripple.triple_count() >= 0 AS library_loaded;
SET search_path TO pg_ripple, public;

-- Helper must exist (installed by CREATE EXTENSION, or repaired by SUM path).
SELECT EXISTS (
    SELECT 1
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname = 'pg_ripple'
      AND p.proname = 'numeric_type_code_spi'
      AND pg_catalog.pg_get_function_identity_arguments(p.oid) = 'bigint'
) AS numeric_type_code_spi_present;

-- Inline integers via VALUES.
SELECT (result->>'s') AS sum_ints
FROM pg_ripple.sparql($$
    SELECT (SUM(?x) AS ?s) WHERE { VALUES ?x { 1 2 3 } }
$$);

SELECT (result->>'a') AS avg_ints
FROM pg_ripple.sparql($$
    SELECT (AVG(?x) AS ?a) WHERE { VALUES ?x { 1 2 3 } }
$$);

-- Dictionary-backed decimals (coach-kg todayMacros shape).
SELECT pg_ripple.load_ntriples(
    '<https://sumagg.test/a> <https://sumagg.test/kcal> "108.0"^^<http://www.w3.org/2001/XMLSchema#decimal> .' || E'\n' ||
    '<https://sumagg.test/b> <https://sumagg.test/kcal> "227.4"^^<http://www.w3.org/2001/XMLSchema#decimal> .' || E'\n' ||
    '<https://sumagg.test/c> <https://sumagg.test/kcal> "297.0"^^<http://www.w3.org/2001/XMLSchema#decimal> .'
) = 3 AS decimals_loaded;

SELECT (result->>'s') AS sum_decimals
FROM pg_ripple.sparql($$
    SELECT (SUM(?kcal) AS ?s) WHERE {
        ?fl <https://sumagg.test/kcal> ?kcal
    }
$$);

SELECT (result->>'a') AS avg_decimals
FROM pg_ripple.sparql($$
    SELECT (AVG(?kcal) AS ?a) WHERE {
        ?fl <https://sumagg.test/kcal> ?kcal
    }
$$);

-- Mixed double + integer promotion stays numeric (not an error).
SELECT pg_ripple.load_ntriples(
    '<https://sumagg.test/d> <https://sumagg.test/score> "1.5E1"^^<http://www.w3.org/2001/XMLSchema#double> .' || E'\n' ||
    '<https://sumagg.test/e> <https://sumagg.test/score> "5"^^<http://www.w3.org/2001/XMLSchema#integer> .'
) = 2 AS doubles_loaded;

SELECT (result->>'s') AS sum_mixed
FROM pg_ripple.sparql($$
    SELECT (SUM(?v) AS ?s) WHERE {
        ?x <https://sumagg.test/score> ?v
    }
$$);

-- COUNT still works (does not need numeric_type_code_spi).
SELECT (result->>'c')::int = 3 AS count_ok
FROM pg_ripple.sparql($$
    SELECT (COUNT(?kcal) AS ?c) WHERE {
        ?fl <https://sumagg.test/kcal> ?kcal
    }
$$);
