# vap

A Rust CLI for Virtuozzo Application Management, Cloud Management and S3 APIs. A single executable, no Java runtime required.

## Build

Rust 1.98 is configured in `rust-toolchain.toml`.

```sh
cargo build --release --locked
# Linux/macOS: target/release/vap
# Windows: target/release/vap.exe
vap --help
```

## Authenticate

Log in with a personal access token:

```sh
vap auth login --platform https://app.example.com --token-file /private/token.txt
vap auth status
```

Or import an existing Jelastic CLI session:

```sh
vap auth import --list
vap auth import --from wsl:Ubuntu
```

Credentials use the OS keychain. On headless systems, explicitly select `--store file`.
Configuration is stored in `~/.vap-cli` (`%USERPROFILE%\.vap-cli` on Windows).

## Usage

```sh
vap env list
vap env info --env my-environment --json
vap projects list --json
vap node list --env my-environment
vap billing quotas
vap logs read --env my-environment --node 123 --param path=/var/log/app.log

# Discover and call API methods
vap api search environment --limit 10
vap api describe environment.control.getenvs
vap api environment control getenvs --json
```

Use `vap <command> --help` for options. Shell completions are available through
`vap completion bash`, `zsh`, `fish`, or `powershell`.

For scripts, use `--json` and explicitly pass `--yes` for modifying operations.
`--read-only` restricts commands to audited read operations. Supply secrets through
files, stdin, `VAP_TOKEN_FILE`, or your CI secret store.

## Development and releases

See the [architecture and extension guide](docs/architecture.md) for adding commands,
request execution, authentication policy and behavioral regression tests.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo install cargo-deny --version 0.20.2 --locked
cargo deny --locked check advisories --warn unmaintained
```

CI checks the release backend (reqwest) on Linux, Windows and macOS, each on x86_64 and ARM64.
Pull requests run CI once. Version tags run CI once through the release workflow before publishing.
RustSec also runs daily; vulnerabilities fail CI, while unmaintained crates produce
warnings. Dependabot checks Cargo and GitHub Actions weekly.

Pushing a tag matching the Cargo version (`vX.Y.Z`) publishes a
[GitHub Release](https://github.com/MikeFalcon77/vap-cli/releases) using the default
reqwest backend. Each platform gets an archive containing `vap` (Linux/macOS) or
`vap.exe` (Windows), a SHA256 checksum and a provenance attestation. Archive names
include the version and target; Windows uses ZIP, Linux/macOS use tar.gz.
The tag must match `Cargo.toml` (`0.1.0` → `v0.1.0`). Releases created in the
GitHub UI receive the same verified assets; existing assets are not overwritten.
Verify a downloaded archive with:

```sh
gh attestation verify <archive> --repo MikeFalcon77/vap-cli
```
