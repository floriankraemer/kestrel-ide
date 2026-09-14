docker.jsonl and podman.json are hand-authored `history` output for the
one image `../inspect/{docker,podman}/images.json` already carries
(`postgres:16-alpine` / `sha256:83a1b4068e0f...`), shaped like real
`docker history --no-trunc --format '{{json .}}'` (NDJSON) and `podman
history --no-trunc --format json` (a JSON array) output — consumed by
`crates/container-core/src/bin/stub_engine.rs` for the E2E fixture, not
captured from a real engine.
