# Local verification — 4 November 2026

This is the initial single-backend build record. Subsequent real-platform tests and the two-backend comparison are documented in [live-validation.md](live-validation.md).

The installed official Jelastic CLI 8.15 API descriptions were normalized into the embedded catalog. `tools/catalog.py` reproduced it exactly from both source exports: **1376 total methods, 422 public, 18 S3**. Source SHA-256 digests are recorded in `assets/provenance.json`. This verifies metadata preservation, not live authorization or behavior of every method.

On native Windows x86_64 and Ubuntu/WSL x86_64:

- 31 tests pass: 19 unit/catalog/transport tests, eight executable CLI tests, four executable HTTPS tests.
- `cargo clippy --locked --all-targets -- -D warnings` and formatting pass.
- Every embedded API command is parsed and its help rendered; every friendly command maps to a real catalog method.
- Local HTTP/HTTPS fixtures verify nested API failures, UTF-8 POST forms, credentials in the request body, CA trust, one request per mutation, bounded read retries, uncertain mutation timeouts, and a 33 MiB template response.
- Executable tests verify consent before legacy import, Java Properties decoding, private file credentials, local-only logout, no secret output, offline preview/schema/completions, profile-scoped refresh and explicit target precedence.
- Readiness tests require a running environment with nodes and verify that polling never repeats the lifecycle change.

Dependency checks pass for all six configured release targets: Linux musl, Windows MSVC and macOS, on x86_64 and ARM64. No AWS-LC, AWS-LC FIPS, OpenSSL or dynamic libdbus crates are selected. Windows uses static CRT; PE imports contain only Windows system DLLs, with no Visual C++ runtime requirement.

Local release artifacts are in `dist/` with SHA-256 and JSON metrics. Windows is a single 2,516,992-byte executable. A local Linux GNU build is also supplied; CI produces musl Linux release builds. Metrics measure local process startup and peak memory for `--help`, including OS overhead; they are not cold-cache benchmarks.

Not established locally: native macOS/ARM64 builds, musl execution, real keychain service roundtrips on every OS, signed/published GitHub releases, or live behavioral parity of every API method. The six-target CI/release workflow is prepared; the initial verification took place before the GitHub remote was configured. Tests use fixtures and temporary credentials, without changing a real Virtuozzo account. Fullscreen TUI remains the subsequent phase agreed in the plan.

## CI setup verification — 5 November 2026

The pinned `1.98` toolchain resolves locally to rustc 1.98.1. On native Windows x86_64, both HTTP backends pass Clippy with warnings denied and all tests (34 reqwest, 32 toolkit-http). Formatting and actionlint pass. Both release builds use `panic = "unwind"`, start successfully and package with SHA-256 checksums and metrics under ignored `dist/ci-verification/`.

Dependency guards pass for all 12 backend/target combinations. `cargo-deny 0.20.2` passes the complete dependency graph using the RustSec database, with three unmaintained warnings in keyring's Linux dependency chain (RUSTSEC-2026-0370, RUSTSEC-2026-0371, RUSTSEC-2026-0372). Unmaintained reports remain warnings; vulnerabilities fail CI, without advisory-ID exceptions. A mismatched version tag is rejected before packaging.

These checks do not publish a release or replace the historical measurements above. Native CI results for the other architectures and operating systems are reported by GitHub Actions.
