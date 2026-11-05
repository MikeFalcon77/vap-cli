# Extending VAP CLI

The provider catalog describes methods and parameters. Trusted command definitions and safety policy live in Rust code. A catalog refresh cannot grant read-only or retry privileges.

## Request flow

1. Bootstrap reads configuration once, selects the profile, and loads its catalog. Clap builds API commands from the catalog and convenience commands from `registry::COMMANDS`.
2. `Selection` resolves the full API path and reads frontend inputs. Pure parameter preparation normalizes aliases, validates input, applies target precedence and invokes the selected command's input handler.
3. `PreparedRequest` validates the path and obtains its immutable `MethodPolicy` from the trusted registry. Neither the caller nor the catalog supplies a retry boolean.
4. `ExecutionContext` holds the selected profile, platform, targets and network settings. Consuming it with `authorize` resolves a credential once. Discovery and preview never authorize. Stored credentials remain bound to their platform origin.
5. `Engine` executes the request and optional lifecycle polling or log snapshots under one absolute deadline. It returns a response or emits events through a callback, without Clap, prompts, stdout or configuration reads.
6. The CLI handles confirmation, private response persistence, redaction, projection and rendering. Reqwest and toolkit-http are adapters beneath the same operation transport.

## Add a command

For an API method, refresh the catalog or use `api call`; no dispatcher change is needed. Catalog presence does not establish live provider support.

For a normal convenience command, add `CommandSpec::new(group, group_help, name, full_api_path)` to `registry::COMMANDS`. The parser and dispatcher consume the same entry. Root commands use an empty group. Missing provider methods remain discoverable and return `METHOD_UNAVAILABLE`.

For extra behavior, attach typed `ExtraArg` definitions, a pure `parameters::Transform` function and/or a response `View` function. File arguments arrive as content read by the frontend. A transform receives parameter and extra-value maps; normal preparation validates the resulting parameters. It does not read profiles, credentials or files. A view receives and returns JSON. Follow environment-create, clone, exec and node-list entries. New handlers do not require dispatcher branches.

Method-wide defaults use the full API path. Environment ReadLog's default count must not affect Marketplace ReadLog; Environment GetEnvs' collection view must not affect administration methods sharing its name.

## Authentication and safety

`session` is reserved for authentication injection. A business parameter named `token` is a separate sensitive input: file/stdin only, redacted on display. For catalogued methods, a required/optional/absent `session` parameter selects required/optional/no session authentication. Unknown raw methods retain required authentication. Missing optional credentials are allowed when no source is configured; errors in configured sources are reported.

Only audited retrieval paths receive `ReadOnly`. Other known lifecycle changes are `Modifying`; remaining paths are `Unaudited`. Both latter categories require confirmation and receive no automatic retries. Add new read privileges only after verifying the operation, together with a CLI-to-wire regression.

## Behavioral tests

Common regressions drive Clap selection and preparation into each real HTTP adapter and decode actual loopback POST bodies. They cover independent ReadLog methods, business tokens versus sessions, permissions, retry counts, deadlines and finite NDJSON streams. Loopback HTTP is available only in test builds; release builds retain HTTPS enforcement.

Executable tests cover partial catalogs, secret inputs, target precedence, origin binding, profiles and offline commands for both backends. Reqwest additionally tests TLS/custom CA. Toolkit-http tests its explicit CA/proxy limitations. Test server accepts, socket IO and CLI subprocesses have deadlines.

Catalog counts and generated help are structural smoke checks, not proof of live behavior for 1376 methods. Real provider checks and lifecycle validation are recorded separately.

## Verification — 5 November 2026

Native Windows x86_64 and Ubuntu/WSL x86_64 GNU passed 43 reqwest tests (28 unit/command-to-wire, 11 executable CLI, four HTTPS) and 41 toolkit-http tests (28 unit/command-to-wire, 11 executable CLI, two capability checks). Clippy passed with warnings denied for both backends. Five deliberately restored bugs were caught on each backend: unsafe stop classification, ReadLog namespace collision, reserved business token, missing-method panic and post-sleep timeout checking. Final native Windows verification used clean package artifacts with incremental compilation disabled after the isolated mutation runs.

Read-only checks using the existing `vhc` profile matched on both native Windows release variants: four environments, five nodes (also checked through `node list` for each environment), one project, two S3 buckets, 100 quota records, nine pricing groups, 44 templates and five regions. This validation created or changed no platform resources.

Release measurements use Rust 1.98.1, size optimization, LTO, one codegen unit, stripped symbols and the repository's current `panic=unwind` policy. Windows uses static CRT. Unpacked executable sizes:

- Windows MSVC: reqwest 3,542,528 bytes; toolkit-http 4,417,536 bytes (+24.70%).
- Linux GNU: reqwest 4,570,832 bytes; toolkit-http 5,762,624 bytes (+26.07%).

These are not directly comparable to earlier Rust 1.97.1/panic-abort builds. Local metrics, checksums and the read comparison are in ignored `dist/` artifacts. Dependency guards passed for all 12 backend/target combinations; FIPS, OpenSSL and dynamic libdbus were absent, and the toolkit build contained no reqwest.

Toolkit-http supports WebPki and native certificate roots. Its missing outbound-proxy and additional-CA configuration is tracked in [gears-rust#5751](https://github.com/constructorfabric/gears-rust/issues/5751); the default backend remains reqwest. Native macOS/ARM64 and Linux musl execution still require the configured CI runners and were not run locally.
