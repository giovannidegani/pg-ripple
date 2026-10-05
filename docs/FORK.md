# giovannidegani/pg-ripple — Overachiever Coach KG fork

**Upstream:** [trickle-labs/pg-ripple](https://github.com/trickle-labs/pg-ripple) (public, default branch `main`)  
**This fork:** [giovannidegani/pg-ripple](https://github.com/giovannidegani/pg-ripple)  
**Consumer:** Overachiever Coach KG (`/workspace/overachiever-coach-kg`) — parallel to the Overachiever app; never tied to the app DB.

## Policy: thin fork

- Keep the fork **thin**: land only patches we need for the coach KG, with clear provenance and tests.
- Improve **as the project goes** — do not wait for upstream, and do **not** depend on the upstream published image long-term.
- Prefer small, reviewable commits; sync from `upstream/main` periodically; upstream PRs when a patch is generally useful.
- Default image tags for our builds: `oa-pg-ripple:dev` (local) and, when GHCR push is configured, `ghcr.io/giovannidegani/pg-ripple:dev`.

## Patch backlog (owned here)

Ordered by priority for the coach KG spike (see also coach-kg `BLOCKERS.md` / `docs/SHACL.md`).

| # | Patch | Why | Notes |
|---|--------|-----|--------|
| 1 | **ORDER BY alpha / lexical sort** | SPARQL `ORDER BY ?var` / `ORDER BY STR(?)` returns dictionary/insertion order in 0.136.0; `DESC` only reverses it. Unusable for UI lists (classes, labels). | First patch to implement. Repro in coach-kg `docs/SPIKE.md`. Sort in SQL (`ORDER BY result->>'…'`) is a temporary workaround only. |
| 2 | **Cross-graph SHACL** | `validate(graph)` is graph-local. Enum / class individuals typed only in the ontology graph cannot satisfy `sh:class` in a tenant graph. | Today we work around with `sh:nodeKind` + `sh:in` for enums. Want: optional import/union graphs or class lookup across configured graphs. |
| 3 | **`load_shacl(graph IRI)`** | In 0.136.0 `load_shacl` takes Turtle **text**, not a graph IRI (some docs imply IRI). We dual-load shapes into a named graph *and* the catalog. | Want: `load_shacl` from an already-loaded named graph IRI so SPARQL and the catalog stay single-source. |
| 4 | **File LOAD allowlist UX** | `*_file` loaders need `pg_ripple.copy_rdf_allowed_paths` **and** paths under PGDATA. Mounted `/kg/…` fails even with allowlist. GUC is `Sighup` (docs sometimes say `Suset`). | Want: clearer errors, documented allowlist+PGDATA contract, and/or safe load-from-bind-mount for local/dev. Coach-kg currently loads via client-side `cat` → `load_turtle_into_graph(text, …)`. |
| 5 | **Write-time SHACL `minCount`** | Offline `validate()` checks `sh:minCount`; sync `insert_triple()` cannot see absence on a single insert. | Want: write-time / deferred checks that enforce required cardinality (batch flush, txn-end validate, or async mode with clear semantics). |

## How we build the image

Upstream publishes via `.github/workflows/release.yml` → `docker/build-push-action` on `Dockerfile` to `ghcr.io/trickle-labs/pg-ripple`. Local equivalent:

```bash
# From repo root (multi-stage: Rust/pgrx pg_ripple + pg_trickle + pg_tide + PostGIS + pgvector → postgres:18-bookworm)
docker build -t oa-pg-ripple:dev .
# optional GHCR tag / push (needs packages:write + docker login ghcr.io)
docker tag oa-pg-ripple:dev ghcr.io/giovannidegani/pg-ripple:dev
echo "$GHCR_TOKEN" | docker login ghcr.io -u giovannidegani --password-stdin
docker push ghcr.io/giovannidegani/pg-ripple:dev
```

Also: `just docker-build tag=dev` → `docker build -t pg-ripple:dev .`

**Deps (see Dockerfile ARGs / `.versions.toml`):** Rust via image (`rust:1-bookworm`; toolchain pin in tree is 1.95.0), `cargo-pgrx` 0.18.0, PostgreSQL 18 headers, clang, PostGIS 3.5.6, pgvector 0.8.2, pg_trickle 0.68.0, pg_tide 0.33.0. Full batteries-included build is large (Rust compile + several extensions) — expect a long first build.

## Coach KG compose

`overachiever-coach-kg/docker-compose.yml` keeps the pinned upstream image as the default (spike stays working) and documents an optional override to this fork's local/`ghcr.io/giovannidegani` image via `build:` / `image:` comments.

## Sync

```bash
git fetch upstream
git merge upstream/main   # or rebase; resolve carefully around our patches
```
