# inspect fixtures

`docker/` is real: captured on 2026-09-13 from Docker Engine 29.6.2 with
`docker inspect`, `docker image inspect`, `docker volume inspect`,
`docker network inspect` and `docker events --format '{{json .}}'`, then
scrubbed: every `Env` value whose key looks like a secret
(PASS/SECRET/TOKEN/KEY/CREDENTIAL/AUTH) and every credential inside a URL
became `<redacted>`, and `/home/florian` became `/home/user`.

`podman/` is hand-authored — podman is not installed on
the capture host. They follow the documented `podman inspect` /
`podman pod ls --format json` / `podman network inspect` shapes (note the
lowercase keys podman's network inspect uses) and the label conventions
`docker compose` (`com.docker.compose.*`) and `podman-compose`
(`io.podman.compose.*`) write. Treat them as the best available
approximation, not as ground truth from a real engine.

Compose grouping is covered by both sets: `docker/containers.json` carries
two real `docker compose` projects (`com.docker.compose.*` labels) and
`podman/containers.json` one `podman-compose` project (`io.podman.compose.*`
labels).
