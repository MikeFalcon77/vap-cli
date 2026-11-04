# vap

A native Rust CLI for Virtuozzo Application Management and related Cloud Management / S3 APIs. One executable per OS and architecture. No JVM or Node.js required.

The embedded CLI 8.15 catalog contains **1376 methods**, including **422 public methods** and **18 S3 methods**. Commands, help, completions and the machine-readable schema are generated from the same metadata. Catalog coverage is tested; live permissions and behavior depend on your platform version and account. This project is an independent client, not an official Virtuozzo CLI.

## Start

```sh
cargo build --release --locked
# Windows: target/release/vap.exe; Linux/macOS: target/release/vap
vap --help
vap api search environment --limit 10
vap api describe environment.control.createenvironment
```

Review existing authentication before importing it:

```sh
vap auth import --list
vap auth import --from wsl:Ubuntu
# Or a local Java Properties file:
vap auth import --from /home/me/.config/jelastic/jelastic.properties
```

Import asks for confirmation and shows the source, platform, profile and store. It never copies passwords. Native Windows builds can discover WSL distributions and import their existing sessions. Multiple sources require `--from`. For automation, inspect sources first and explicitly pass `--yes --json`.

Alternatively, use a personal access token:

```sh
vap auth login --platform https://app.example.com --token-file /private/token.txt
vap auth status
```

Without `--token-file`/`--token-stdin`, login uses a hidden terminal prompt. `--password-login --email user@example.com` uses hidden password input; `--password-stdin` supports headless input. Accounts requiring 2FA should use a PAT. A PAT is stored without attempting to claim server validation. Imported sessions retain their original expiry and cannot refresh automatically.

Configuration is under `~/.vap-cli` (`%USERPROFILE%\.vap-cli` on Windows). Credentials use Windows Credential Manager, macOS Keychain, or Linux Secret Service. No file fallback happens automatically. On servers without keychain, explicitly choose `--store file`; files use `0600`, directories `0700`, and protected owner-only Windows ACLs. File storage does not encrypt credentials at rest. `auth logout` removes only the local copy; `auth revoke --yes` additionally revokes a server session and affects other clients using that same session. PAT revocation is performed in the platform's token management UI.

## Common operations

```sh
vap env list
vap env info --env my-environment --json
vap projects list --json
vap s3 list --project 123 --json
vap node list --env my-environment
vap billing quotas
vap billing pricing --json
vap logs list --env my-environment --node 123
vap logs read --env my-environment --node 123 --param path=/var/log/app.log
vap logs watch --env my-environment --node 123 --param path=/var/log/app.log --samples 10 --json
vap operations list --env my-environment --json
```

`--project` is the Cloud Management **engineProjectId** returned by the project API. The client never guesses among projects. Log reads default to 100 lines; watch is a finite series of snapshots, with unchanged snapshots suppressed. JSON watch emits one versioned JSON envelope per snapshot (NDJSON).

Preview a topology without sending any request:

```sh
vap env create --name my-new-env --input topology.json --disk-gb 5 --preview --json
vap env create --name my-new-env --input topology.json --disk-gb 5 --yes --wait
```

`topology.json` is a JSON object with `env` and `nodes` API parameters. For example:

```json
{
  "env": { "region": "eu1" },
  "nodes": [{ "nodeType": "nginxphp-dockerized", "nodeGroup": "cp", "count": 1, "fixedCloudlets": 1, "flexibleCloudlets": 4, "extip": 0 }]
}
```

Select valid stack/region settings for your provider with `env templates` and `env regions`. `--disk-gb` is **GB**, not MB. `fixedCloudlets` and `flexibleCloudlets` are platform cloudlet counts. Pricing output is the provider's tariff data, not a monthly guarantee or spending cap. Preview constructs and validates the local request; it is not server validation or a cost estimate.

`--wait` supports environment create/clone/start/stop/delete. It polls actual environment state; a creation acknowledgement or an empty provisioning node list is not completion. `--timeout` is a deadline in seconds, default 600; use a larger value for slow provisioning. Ctrl+C stops local waiting and returns 130, without claiming to cancel the server operation. Inspect state before retrying any timeout. Modifying calls are never retried automatically. Only an explicit audited retrieval allowlist can retry, up to two times within the request deadline, honoring numeric Retry-After headers.

Use file/stdin input for secrets and opaque payloads, including variables, commands and file bodies:

```sh
vap vars set --env my-environment --node 123 --input variables.json --yes
vap exec --env my-environment --node 123 --command-file command.txt --yes
vap files write --env my-environment --node 123 --input write-request.json --yes
```

`files upload` invokes the platform API's sourcePath/destPath semantics; it is not an SCP client. Build/deploy pipelines and S3 data-plane sync remain separate tools. Values stored inside Docker Engine/Compose applications are not automatically managed by the outer VAP node variables.

## Every API method

```sh
vap api environment control getenvs --json
vap api environment control getenvinfo --env-name my-environment --json
vap api s3 bucket list --project-id 123 --json
vap api search bucket --json --limit 10
vap api describe s3.bucket.create --json
vap api schema --json
vap api call future/service/rest/newmethod --input request.json --yes --json
```

Method parameter flags use kebab-case (`nodeId` → `--node-id`); provider aliases are accepted where they do not collide. `--param NAME=VALUE` uses original API parameter names. Integers and booleans are parsed according to metadata. Objects and arrays are preserved and JSON-encoded into the POST form. There is no double encoding of JSON strings. Optional values should be omitted, not passed as null. Opaque payloads and secret fields require JSON file/stdin input; there are no password/token argv flags. Authentication is always injected and input cannot override `session`.

All requests use POST over verified HTTPS; credentials are not put in URLs. Redirects are not followed. The default reqwest backend supports environment/system proxies and `--ca additional-ca.pem`. `VAP_MAX_RESPONSE_BYTES` sets the decompressed response limit (64 MiB by default), independent of the old Java wrapper's heap limit.

Refresh metadata explicitly from a provider JSON export:

```sh
vap api schema refresh --file private_api.json --platform-version 8.15.1 --yes --json
```

This replaces the selected profile's catalog and reports added, removed and parameter-changed methods. It never silently performs a network refresh. Pass a complete export when retaining extended methods matters. Public/extended metadata does not confer permissions. Unknown/new methods remain reachable using `api call`; unknown operations conservatively require confirmation. Use `api describe` for required parameters and source version. The original embedded provenance is in [assets/provenance.json](assets/provenance.json).

## Agents and scripts

- `--json` implies no prompts. `--non-interactive` also disables prompts. Pipe/non-TTY/CI input never prompts.
- Changes or operations not audited as read-only require `--yes`. `--read-only` rejects those operations even with `--yes`.
- Stdout contains data, stderr contains diagnostics/progress. ANSI/progress is disabled for JSON, pipes, `NO_COLOR` and dumb terminals. Help/version and completion intentionally emit their requested text rather than JSON.
- JSON uses `{ "schema_version": 1, "ok": true, "data": ..., "error": null, "meta": ... }`. Errors use the same envelope with `ok: false`, an error code, optional API result code and hint. Known nested `response.result` failures are errors, even if the outer result is zero.
- `--fields env.envName,env.status` projects returned items; `--limit 20` truncates displayed collections and reports `truncated`, `total` and `returned`. Human tables summarize scalar columns; JSON retains full structures unless projected.
- Provider pagination is explicit: use the method's `offset`/`count`, `startRow`/`resultCount` or `maxKeys`/`continuationToken` parameters as documented by `api describe`. The client does not implicitly fetch every page. Continuation tokens can be supplied through file/stdin.
- `--output private-response.json` saves the **unredacted** complete successful API response privately and refuses overwrite; this supports one-time keys. Displayed output redacts recognized secret fields and known input credentials. Treat arbitrary application logs/file contents as potentially sensitive too.
- `VAP_PLATFORM` plus `VAP_TOKEN` or `VAP_TOKEN_FILE` provides headless authentication without persisting credentials. Supply secrets through a CI secret store; do not embed their values in shell commands. `VAP_TOKEN` takes precedence over `VAP_TOKEN_FILE`, then the profile credential.
- `--profile` / `VAP_PROFILE` selects a profile. `profile use NAME` sets the default; `profile set --env NAME --project ID --node ID` saves target defaults. Explicit target flags override request-file targets, then request targets override saved defaults. Stored credentials cannot be sent to an overridden platform origin.

Exit codes: 0 success, 1 API/HTTP/storage/other operation failure, 2 invalid input/unknown method/ambiguous target, 3 missing/expired authentication, 4 permission/confirmation required, 5 network failure, 6 deadline/unknown outcome, 130 interrupted. Never infer safe retry from exit status alone.

Generate completions offline with `vap completion bash`, `zsh`, `fish`, or `powershell`. `vap doctor --json` reports build/configuration/catalog information without credentials. No telemetry, background update checks, embedded agent model, or background daemon is enabled.

## Validation and releases

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo deny --locked check advisories --warn unmaintained
python tools/check_dependencies.py --target x86_64-pc-windows-msvc
```

Tests cover every catalog command, secret inputs/import consent, private file storage, Java Properties escaping, nested API errors, request serialization, retries, uncertain modification timeouts, readiness transitions and responses larger than the Java 32 MiB heap. Tests use local fixtures/mock servers; they do not provision or delete real platform resources.

CI runs formatting, Clippy, dependency guards and native tests for both HTTP backends on Linux musl, Windows MSVC and macOS, each on x86_64 and ARM64 (12 combinations). The separate release workflow accepts tags matching the Cargo package version (`vX.Y.Z`), reruns CI and builds single-file artifacts with checksums, startup/size/memory metrics and Sigstore-backed GitHub provenance attestations. Verify with `gh attestation verify <binary> --repo MikeFalcon77/vap-cli`. The release workflow requires a repository plan supporting artifact attestations; it fails rather than publishing an unattested release. This does not provide Apple notarization or Windows Authenticode signing. A Rust toolchain and Python are build/CI tools only, not runtime requirements. Linux keychain storage uses the OS Secret Service if available; explicit file storage works headlessly.

Full live behavioral parity across all methods, real keychain roundtrips on each OS, and platform-specific release CI results must be established against the intended providers/runners. No such live coverage is implied by the metadata inventory. A fullscreen TUI is a subsequent phase; the current release focuses on CLI/API reliability.

RustSec runs on pushes, pull requests, manual CI runs, releases and daily at 06:00 UTC. Install the pinned checker with `cargo install cargo-deny --version 0.20.2 --locked`. Known vulnerabilities fail CI; unmaintained crates remain visible warnings without advisory-ID exceptions. The full dependency graph includes both HTTP backends and all platforms. Dependabot checks Cargo and GitHub Actions weekly on Monday at 09:00 Europe/Sofia and opens update PRs without automatic merging.

## HTTP backend variants

The default remains `reqwest-client` with rustls/ring. The experimental `toolkit-http-client` uses the exact published `cf-gears-toolkit-http 0.8.0` with FIPS disabled and ordinary AWS-LC. It contains no reqwest dependency. Select exactly one backend:

```sh
cargo +1.98 build --release --locked --no-default-features --features reqwest-client
cargo +1.98 build --release --locked --no-default-features --features toolkit-http-client
cargo +1.98 test --locked --no-default-features --features toolkit-http-client --all-targets
python tools/check_dependencies.py --target x86_64-pc-windows-msvc --backend toolkit-http
```

Toolkit-http 0.8.0 requires Rust 1.95 or newer. The historical local comparisons used 1.97.1; development and CI now use the pinned 1.98 toolchain for both variants. Both adapters share VAP serialization, error handling, read-only retry classification, deadlines, response limits and output. Toolkit-http's own retries, redirects and optional telemetry are disabled. Its body limit is explicitly raised to the same 64 MiB default.

The published toolkit HttpClientBuilder does not expose custom CA injection, proxy connector configuration, or an independent connect timeout. The variant rejects `--ca` and explicitly configured proxy environment variables with `UNSUPPORTED_FEATURE`; it never disables TLS verification. It does not implement system proxy discovery. Consequently it is an experimental alternative, rather than a feature-identical replacement for the default. Tests requiring custom CA run on reqwest; toolkit tests verify explicit rejection. Both share the remaining CLI/transport tests and were compared against the same real platform's read APIs.

The dependency guard rejects FIPS/OpenSSL/dynamic libdbus for both, and also rejects ordinary AWS-LC for the reqwest variant. This follows the requested prohibition on **FIPS AWS-LC**, while allowing ordinary AWS-LC for the gears comparison.

Native artifacts are named `vap-reqwest-<target>` and `vap-toolkit-http-<target>` (plus `.exe` on Windows). `tools/package.py --backend ...` records unpacked size, SHA-256, compiler, help startup and help peak memory. `tools/compare_backends.py` compares those metrics. Build settings are identical: size optimization, LTO, one codegen unit, stripped symbols, panic unwind, and static Windows CRT.

Live test details are recorded in [docs/live-validation.md](docs/live-validation.md). An opt-in `tools/live_smoke.py` harness journals one newly-created environment and checks its identity before each mutation; it never treats an existing environment as disposable. The `.env` file used for explicit password input is ignored by Git. DeleteEnv's `password` field can mean an environment-name confirmation when the current `environment.delete.confirm.type` quota is 1; the live run verified this mode. The imported session remains in the system keychain.
