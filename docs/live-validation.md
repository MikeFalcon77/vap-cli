# Live validation and HTTP backend comparison

## Real platform checks

The current WSL session was imported into the `vhc` profile and Windows Credential Manager. The native Windows CLI then authenticated to `https://app.vhc.virtuozzo.com`. Session/password values were never command arguments or report content. `.env` and its variants are ignored by Git.

The baseline contained four environments, five nodes, one default Cloud Management project, and two S3 buckets. Read-only checks covered environment/node discovery, projects, both bucket object listings, 100 quota records, nine pricing groups, 44 stack templates and five regions. The project was explicitly selected with its `engineProjectId`.

The isolated environment `vap-cli-smoke-f8ab3933a9a6` used the platform's standard NGINX/PHP application-node template (`nginxphp-dockerized`), not a Docker Engine installation. Its requested topology was one node in `eu1`, one fixed cloudlet, four maximum cloudlets, 5 GB disk and zero external IPs. Provisioning reached running state with a node after **149.805 seconds**; the create request was never repeated.

Inside that newly-created environment, the reqwest variant verified:

- Command execution and recognizable stdout, before and after stop/start.
- File creation, UTF-8 readback, append, rename, listing, deletion and an explicit absence check under a unique `/tmp` path.
- Setting and reading a unique dummy environment variable, then removing it and verifying absence. Unredacted variable responses were read only from private temporary files and removed immediately.
- Vertical scaling from four to five maximum cloudlets, readback, and restoration to four.
- Environment stop/start with `--wait` and observed state transitions.
- A missing-file API error: result **2002**, process exit **1**, structured error JSON.
- Environment deletion with `--wait` and independent absence verification.

The actual node API reported `diskLimit=5000` for a creation input of `diskLimit=5`: these specific endpoints return MB and accept GB respectively. No implicit conversion was applied to the request.

The current account quota `environment.delete.confirm.type=1` specifies confirmation by **environment name**. DeleteEnv accepted the environment name in its schema's `password` field, so the account password was not needed. This differs from the earlier pilot's assumption that an account password was always necessary. No quota or security-policy setting was modified.

Deletion completed in **2.294 seconds**. The disposable environment disappeared; the original four environment names, all five original node IDs and both original bucket names were independently rechecked and preserved. The run journal records **35 checks: 34 successes and one expected missing-file error**. There are no remaining test resources from this run.

## Backend comparison method

Two release builds use the same application, catalog, credential handling, serializer, response validation and output code. The HTTP adapter selects either reqwest 0.13.5 with rustls/ring, or the exact published cf-gears-toolkit-http 0.8.0 with ordinary AWS-LC. FIPS is disabled and absent from the selected dependency graph for both. The toolkit build does not contain reqwest.

Both are built using Rust **1.97.1**, `opt-level=z`, LTO, one codegen unit, stripped symbols and panic abort. Windows uses static CRT in both. Linux measurements use native x86_64 GNU builds in WSL, not the future musl CI artifacts. Measured size is the unpacked executable, without UPX or an installer.

Windows measurements:

- reqwest: **2,525,184 bytes**.
- toolkit-http: **3,315,712 bytes**.
- toolkit-http increase: **790,528 bytes / 31.31%**.

Linux x86_64 GNU measurements:

- reqwest: **3,926,008 bytes**.
- toolkit-http: **5,095,592 bytes**.
- toolkit-http increase: **1,169,584 bytes / 29.79%**.

Each backend independently read the same real platform's environments, projects, S3 buckets, quotas, complete pricing models, templates and regions. All seven stable-field comparisons passed. Quota records preserve duplicate names at different policy scopes; response order and tariff collection order are normalized for comparison. The scripts never collapsed quotas into an order-dependent last-wins map.

Artifacts, exact compiler strings, checksums and Linux measurements are recorded in `dist/vap-<backend>-<target>.metrics.json`; the combined report is `dist/http-backend-comparison.json`. `tools/compare_backends.py --live-profile vhc` reproduces the read-only comparison. Help startup and memory measurements are diagnostic only, include OS overhead, and do not measure HTTP throughput or cold-cache startup.

## Capability differences

The published toolkit-http 0.8.0 HttpClientBuilder does not expose custom CA injection, proxy connector configuration or a separate connect timeout. The experimental adapter explicitly rejects `--ca` and configured proxy environment variables; it does not implement OS proxy discovery. TLS verification remains enabled. The default reqwest variant retains those capabilities.

Toolkit-http's built-in retries, redirects, rate limiter and optional telemetry are disabled. VAP owns the same audited-read retry policy and outer deadline for both. The response limit is set to 64 MiB, including for toolkit's limited body reader. Common mock tests verify POST encoding, nested/batch errors, no retries for mutations, read retries, mutation timeouts, large responses and disabled redirect following.

The custom-CA HTTPS integration tests run on reqwest. Toolkit has explicit unsupported-CA/proxy tests instead, plus real publicly trusted HTTPS read checks. Actual lifecycle mutations were exercised on the original reqwest variant, not repeated on toolkit merely for the size comparison. This comparison therefore does not assert full feature parity or full live coverage of all 1376 catalog methods. The default backend remains reqwest; toolkit-http is a buildable alternative for evaluation.

## Local automated verification

Both native Windows x86_64 and Ubuntu/WSL x86_64 GNU passed **34 reqwest tests** (22 unit/catalog/transport, eight executable CLI, four HTTPS) and **32 toolkit-http tests** (the same 22 unit/catalog/transport and eight CLI tests, plus two explicit capability-rejection tests). Both backends passed Clippy with warnings denied on both systems. Formatting and Python syntax checks passed. Dependency guards passed for both backends on all six configured release targets; no FIPS, OpenSSL or dynamic libdbus dependency was selected.

The other native architectures, macOS and Linux musl execution still require the prepared CI matrix; local GNU measurements do not establish their artifact sizes or runtime behavior.
