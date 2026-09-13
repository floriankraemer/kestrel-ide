podman is not installed on the host that produced these fixtures (only
`docker` is). `podman_connection_list.json` and `podman_machine_list.json`
in this directory, and `../probe/podman_version.json`, are hand-authored
from the documented output shapes (`podman system connection list --format
json`, `podman machine list --format json`, `podman version --format
json`) rather than captured from a real `podman` binary. `docker_version.json`
and `docker_context_ls.jsonl` next to them are real, captured with
`docker version --format json` / `docker context ls --format json` on the
host.
