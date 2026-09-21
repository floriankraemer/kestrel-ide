# 0061. Database security posture: keychain-only secrets, CLI SSH tunnels, rustls defaults, driver-download trust

## Status

Proposed.
Implemented across [the database-tools plan](../database-tools-plan.md)'s F1 (secrets, TLS defaults), F4 (DML safety, security-audit gate), F7 (SSH fallback) and F8 (driver downloads, quarantine) phases.
Reviewed by the security-expert agent before F1 and again before F8, per the plan's §6.

## Context

Database Tools introduces four attack surfaces this codebase has not had before: credentials for arbitrary external systems, a tunnel into a private network, foreign machine code loaded at runtime (an ADBC or ODBC driver), and a data-editing path that constructs SQL from user and schema-derived input.
Each needed a stated posture before F1 could start, not discovered ad hoc as each phase happened to touch it.

## Decision

### 1. Secrets never touch `settings.toml`; the OS keychain is the only store

A data source's password, its SSH passphrase, and its client-TLS key password all go through `secret-store` (the extraction described in [ADR-0058](0058-database-driver-seam.md) §5), keyed `ide.database` / `<id>`, `<id>/ssh`, `<id>/ssl-key`.
`[database.sources]` in `app-config`'s TOML has structurally no field for any of the three — not "empty by convention," the type has no such member — so a settings file, however it was produced or edited, cannot carry a credential even by accident.
A locked-down environment where the keychain itself is unavailable (the same `SecretError::Unavailable` case `container-registry` already handles) degrades to a session-only prompt: the user is asked every time, nothing is written to disk as a fallback.
History (`<config_dir>/database/history/<id>.jsonl`) stores the statement text a user ran, never the bound parameter values that statement carried — a logged `SELECT * FROM users WHERE ssn = $1` never becomes a logged SSN.
`ConnectSpec`'s `Debug` implementation redacts every credential field, so a panic log or an error message built from `{:?}` can never leak one.

### 2. TLS defaults to `Prefer`, tightens to `VerifyFull` once a CA is configured, and never silently skips verification

Every native driver connects through rustls with the `ring` crypto provider (not `aws-lc-rs` — see the cross-link spike finding in the plan's §13/§7 R2, which found this needs active per-crate feature work, not a passive assumption).
`SslConfig`'s modes are `Disable | Prefer | Require | VerifyCa | VerifyFull`; the default for a new data source is `Prefer` (opportunistic TLS on a TCP connection, matching most engines' own defaults), and `VerifyFull` becomes the effective default the moment a CA certificate is configured for that source — a user who went to the trouble of supplying a CA is assumed to want it checked.
There is no "skip certificate verification" flag anywhere in v1: `Disable` means no TLS was ever negotiated, not "TLS negotiated, certificate ignored" — the two are different risks and this codebase does not offer the second one at all, on the same reasoning ADR-0053 gives for preferring an honest `git status --porcelain=v2` read over a faster but silently-wrong shortcut.

### 3. Read-only is enforced twice: client classification and the server's own session flag

A source marked read-only refuses a `Write`/`Ddl`-classified statement client-side (`db_sql::classify`, ≥ 40 test cases per dialect including CTE-wrapped DML, `SELECT … INTO`, and a bare `CALL`) *and* sets the engine's own read-only session flag where the engine has one (`SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY` and equivalents, `dialect::tx_read_only_stmt`).
Neither alone is trusted as sufficient: a classifier can miss a dialect quirk, and not every engine has a server-side flag to fall back on, so the two together are the actual guarantee, not either in isolation.
The data editor is *hidden* on a read-only source, not merely disabled-and-greyed — the same "an unsupported capability is absent, not a disabled button the user has to wonder about" rule `dap-core` already applies to a debug adapter's undeclared capabilities.

### 4. ADBC/ODBC drivers are foreign machine code; downloads are pinned, hashed, quarantined, and never auto-updated

Loading an ADBC shared library or resolving an ODBC driver path means executing code this repository did not write and cannot sandbox — the same blast radius `syntax_core::runtime`'s downloadable tree-sitter grammars already carry, and the same defence applies: a quarantine marker (`<config_dir>/database/drivers/.quarantine/<id>`) wraps the *first* load of any newly installed driver, the same "a crash on first use of new foreign code disables that one thing, not the app" shape a wasm plugin's fuel/epoch trap already gives `plugin_host::wasm` (ADR-0028), applied here to native code instead of a sandboxed guest.
Every built-in driver's download URL is `https://` only and carries a sha256 shipped in the plugin's own manifest (`plugin-api` validates the hex format; it cannot validate that the hash is *correct*, only that it is well-formed — the actual download-time comparison is `db-driver-adbc::install`'s job, and a mismatch deletes the downloaded file and returns a typed `DriverChecksum` error, never a "loaded anyway" fallback).
A consent dialog names the publisher, the URL, and the hash before the first download of any driver — nothing downloads silently.
`allow_third_party_drivers = false` is the shipped default: an installed plugin (not one of this codebase's own built-ins) may declare a `database-drivers` row, but that row cannot trigger a download unless the user opts in — a plugin the user installed for an unrelated reason cannot pull arbitrary native code onto their machine as a side effect of a database driver row it happened to also declare.
No driver auto-updates: a version bump is exactly as deliberate an action, by the user, as the first install was.

### 5. SSH tunnel: the CLI first, `russh` only where the CLI cannot do the job

`ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes -L 127.0.0.1:<port>:<host>:<port> user@bastion` is the default tunnel mechanism (ADR-0055's argument restated for a second feature: the user's own `~/.ssh/config`, agent, FIDO token and `ProxyJump` chain come free from a real `ssh` binary, at zero reimplementation cost).
`BatchMode=yes` means a tunnel that would need interactive input fails fast rather than hanging the connect flow; host-key checking is left at the user's own `ssh` policy, untouched — this feature does not loosen or bypass it; the forwarded port binds to `127.0.0.1` only, never a wildcard address a different local process or network peer could reach.
`russh` 0.63 (pure Rust, `ring`) is the in-process fallback for exactly the two cases the CLI cannot cover: no OpenSSH binary present (a bare Windows install), or a bastion that needs a password/passphrase `BatchMode=yes` refuses to supply interactively.
`russh`'s known-hosts handling reads the user's real `~/.ssh/known_hosts` and prompts on an unrecognised host key with its fingerprint — it never auto-accepts an unknown key, matching the CLI path's own (unmodified) strictness rather than being a weaker fallback.

### 6. Dump/restore tools receive credentials through environment or a mode-restricted file, never argv

`pg_dump`/`mysqldump`/`mongodump` (and their restore counterparts) receive a password via `PGPASSWORD`/`MYSQL_PWD` or a `chmod 0600` defaults file, never as a command-line argument — argv is visible to every other process on the same machine via `/proc/<pid>/cmdline` or Task Manager, an environment variable set only for the child process is not.

### 7. Imports are parameterised and batched; XLSX reads only cached values

A CSV/XLSX import becomes a sequence of parameterised, batched `INSERT`s built the same way the data editor's own `DmlPlan` is — never string-interpolated SQL — and an XLSX import reads only a cell's cached computed value (`calamine` does not evaluate formulas), so an imported spreadsheet cannot smuggle a formula that executes anything on read.

## Alternatives considered

| Option | Why rejected |
|---|---|
| `russh` as the only SSH mechanism (skip the CLI) | Throws away the user's own `~/.ssh/config`, agent and FIDO/`ProxyJump` support for free, for no security benefit — the CLI path is not less secure, it is more capable, and `russh` earns its place specifically as the fallback for what the CLI cannot do, not a replacement for what it can. |
| Auto-updating drivers (check for a newer ADBC driver version periodically) | A driver update is exactly the kind of foreign-code change that deserves the same explicit consent as the first install — an automatic background update of code that gets loaded into the process is a bigger, quieter attack surface than the manual-install path this ADR otherwise pins down carefully. |
| A "skip TLS verification" checkbox | Every real-world case for wanting one (a self-signed cert during local development) is already served by `Disable` (no TLS) or by importing the self-signed cert as a configured CA and using `VerifyCa`/`VerifyFull` against it — a dedicated bypass flag would be the one path in this feature that looks secure (a green padlock, a negotiated TLS session) while actually verifying nothing, which is a worse failure mode than plainly having no TLS at all. |

## Consequences

- A support question of the shape "where did my password go" has exactly one answer — the OS keychain, or nowhere if it was unavailable — never "check `settings.toml`" or "check the history file."
- The read-only guarantee is provable by construction (two independent checks) rather than by code review alone, and is exactly the property the plan's NFR table (ADR-0058) requires nightly negative tests per engine to keep proving.
- A future third native-driver's TLS setup inherits `ring`-only by construction only if that driver crate's own feature selection is audited against `aws-lc-rs` leaking in — the P0 cross-link spike found this does not happen automatically via a workspace-level `rustls` override, so this is an active per-crate task in F1/F7/F8, tracked as R2 in the plan, not a property this ADR can claim is already enforced.
- ADBC/ODBC's quarantine marker and consent dialog mean the first-run experience of a new driver is deliberately one click slower than IntelliJ's own silent driver auto-download — accepted, since the alternative is silent execution of unreviewed native code.
- `security-audit` is a named gate before F4 merges (the data editor, where user input first becomes SQL bound for execution) and before F8 merges (driver downloads and quarantine) — not a courtesy pass, a blocking review the plan's task list already records.

## Related

- [ADR-0058: database driver seam](0058-database-driver-seam.md) — the `secret-store` extraction and the `rustls`/`ring` pin this ADR's §1/§2 build on.
- [ADR-0021: AI chat](0021-ai-chat.md) — the environment-only-credential and `keyring` Linux-backend precedent §1/§7's reasoning reuses.
- [ADR-0028: wasm plugin tier](0028-wasm-plugin-tier.md) — the "a trap disables one thing, never the process" shape §4's quarantine marker applies to native code instead of a sandboxed guest.
- [ADR-0041: `dap-core`](0041-dap-core.md) — the "an unsupported capability is absent, not disabled" rule §3 applies to the data editor on a read-only source.
- [ADR-0053: working-tree status via `git status --porcelain=v2`](0053-git-status-via-porcelain-v2.md) — the "an honest, verified answer over a faster but silently-wrong one" reasoning §2 restates for TLS verification.
- [ADR-0055: CLI-driven container integration](0055-cli-driven-container-integration.md) — the CLI-first-native-fallback-second argument §5's SSH tunnel restates for a second feature.
- `docs/architecture/database-tools-plan.md` — the plan whose §6 states this posture and whose §13 records the cross-link spike finding this ADR's consequences reference.
