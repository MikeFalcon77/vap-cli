#![cfg(feature = "toolkit-http-client")]
mod support;
use std::process::{Command, Stdio};

#[test]
fn proxy_is_rejected_without_echoing_proxy_credentials() {
    let root = tempfile::tempdir().unwrap();
    let out = support::wait_output(
        Command::new(env!("CARGO_BIN_EXE_vap"))
            .args(["env", "list", "--platform", "https://localhost:1", "--json"])
            .env("VAP_CONFIG_DIR", root.path())
            .env("VAP_TOKEN", "FAKE_SENTINEL")
            .env("HTTPS_PROXY", "http://user:PROXY_SECRET@localhost:2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "UNSUPPORTED_FEATURE");
    for secret in ["FAKE_SENTINEL", "PROXY_SECRET"] {
        assert!(!String::from_utf8_lossy(&out.stdout).contains(secret));
    }
}

#[test]
fn unsupported_ca_is_explicit_and_never_disables_verification() {
    let root = tempfile::tempdir().unwrap();
    let output = support::wait_output(
        Command::new(env!("CARGO_BIN_EXE_vap"))
            .args([
                "env",
                "list",
                "--platform",
                "https://localhost:1",
                "--ca",
                "does-not-exist.pem",
                "--json",
            ])
            .env("VAP_CONFIG_DIR", root.path())
            .env("VAP_TOKEN", "FAKE_SENTINEL")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "UNSUPPORTED_FEATURE");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("FAKE_SENTINEL"));
}
