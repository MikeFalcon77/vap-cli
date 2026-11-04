mod support;
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn invoke(root: &tempfile::TempDir, args: &[&str], input: Option<&str>) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vap"))
        .args(args)
        .env("VAP_CONFIG_DIR", root.path())
        .env_remove("VAP_PROFILE")
        .env_remove("VAP_TOKEN")
        .env_remove("VAP_TOKEN_FILE")
        .env_remove("VAP_PLATFORM")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    support::wait_output(child)
}
fn json_output(
    root: &tempfile::TempDir,
    args: &[&str],
    input: Option<&str>,
) -> (std::process::Output, Value) {
    let out = invoke(root, args, input);
    let v = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid stdout: {} / {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, v)
}

#[test]
fn offline_discovery_and_errors() {
    let root = tempfile::tempdir().unwrap();
    let (out, v) = json_output(
        &root,
        &["api", "search", "s3", "--json", "--limit", "3"],
        None,
    );
    assert!(out.status.success());
    assert_eq!(v["data"].as_array().unwrap().len(), 3);
    assert_eq!(v["meta"]["catalog"]["methods"], 1376);
    assert_eq!(v["meta"]["total"], 18);
    assert_eq!(v["meta"]["truncated"], true);
    assert!(out.stderr.is_empty());
    let (out, v) = json_output(
        &root,
        &["--json", "--password", "FAKE_SECRET_SENTINEL"],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(v["error"]["code"], "INVALID_INPUT");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE_SECRET_SENTINEL"));
    assert!(!root.path().join("config.json").exists());
}

#[test]
fn preview_is_offline_and_accepts_aliases() {
    let root = tempfile::tempdir().unwrap();
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "environment",
            "control",
            "getenvinfo",
            "--domain",
            "example",
            "--json",
            "--preview",
        ],
        None,
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["parameters"]["envName"], "example");
    assert_eq!(v["meta"]["sent"], false);
    let (out, v) = json_output(
        &root,
        &[
            "env",
            "create",
            "--name",
            "new",
            "--disk-gb",
            "100",
            "--input",
            "-",
            "--preview",
            "--json",
        ],
        Some(
            r#"{"env":{},"nodes":[{"nodeType":"docker","fixedCloudlets":16,"flexibleCloudlets":128}]}"#,
        ),
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["parameters"]["nodes"][0]["diskLimit"], 100);
    assert_eq!(v["data"]["parameters"]["env"]["shortdomain"], "new");
}

#[test]
fn secret_payloads_require_file_or_stdin() {
    let root = tempfile::tempdir().unwrap();
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "users",
            "authentication",
            "signin",
            "--param",
            "password=FAKE_SECRET_SENTINEL",
            "--param",
            "login=user@example.com",
            "--preview",
            "--json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(v["ok"], false);
    assert!(!out.stdout.windows(20).any(|s| s == b"FAKE_SECRET_SENTINEL"));
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "users",
            "authentication",
            "signin",
            "--input",
            "-",
            "--preview",
            "--json",
        ],
        Some(r#"{"login":"user@example.com","password":"FAKE_SECRET_SENTINEL"}"#),
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["parameters"]["password"], "[REDACTED]");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE_SECRET_SENTINEL"));
    let (out, v) = json_output(
        &root,
        &[
            "env",
            "delete",
            "--env",
            "example",
            "--read-only",
            "--preview",
            "--json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(v["error"]["code"], "PERMISSION_DENIED");
}

#[test]
fn legacy_import_requires_consent_and_keeps_passwords_out() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("legacy.properties");
    std::fs::write(&source,"jelastic.cli.platform_url=https\\://example.com\njelastic.cli.session=FAKE_SESSION_SENTINEL\npassword=NEVER_IMPORT_THIS\n").unwrap();
    let path = source.to_str().unwrap();
    let (out, v) = json_output(
        &root,
        &[
            "auth", "import", "--from", path, "--store", "file", "--json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(v["error"]["code"], "CONFIRMATION_REQUIRED");
    assert!(!root.path().join("config.json").exists());
    let (out, v) = json_output(
        &root,
        &[
            "auth", "import", "--from", path, "--store", "file", "--yes", "--json",
        ],
        None,
    );
    assert!(out.status.success(), "{v}");
    let conf = std::fs::read_to_string(root.path().join("config.json")).unwrap();
    assert!(!conf.contains("FAKE_SESSION_SENTINEL"));
    assert!(!conf.contains("NEVER_IMPORT_THIS"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE_SESSION_SENTINEL"));
    let secrets = std::fs::read_dir(root.path().join("credentials"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(secrets.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&secrets[0]).unwrap(),
        "FAKE_SESSION_SENTINEL"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&secrets[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let (out, v) = json_output(&root, &["auth", "status", "--json"], None);
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["credential_available"], true);
    let (out, v) = json_output(&root, &["auth", "logout", "--json"], None);
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["server_session_revoked"], false);
    assert!(!secrets[0].exists());
}

#[test]
fn targets_and_raw_paths_are_safe() {
    let root = tempfile::tempdir().unwrap();
    for path in [
        "https://evil.test",
        "../foo/rest/bar",
        "users/authentication/rest/signin?password=x",
    ] {
        let (out, _) = json_output(&root, &["api", "call", path, "--json", "--preview"], None);
        assert_eq!(out.status.code(), Some(2));
    }
    let (out, v) = json_output(
        &root,
        &["s3", "list", "--project", "673", "--json", "--preview"],
        None,
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["parameters"]["projectId"], 673);
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "call",
            "future/service/rest/newmethod",
            "--input",
            "-",
            "--json",
            "--preview",
        ],
        Some(r#"{"options":{"numbers":[1,2]}}"#),
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["parameters"]["options"], json!({"numbers":[1,2]}));
}

#[test]
fn completions_and_missing_input_do_not_prompt() {
    let root = tempfile::tempdir().unwrap();
    let bare = invoke(&root, &[], None);
    assert!(bare.status.success());
    assert!(
        String::from_utf8_lossy(&bare.stdout).contains("Usage:")
            || String::from_utf8_lossy(&bare.stderr).contains("Usage:")
    );
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let out = invoke(&root, &["completion", shell], None);
        assert!(out.status.success());
        assert!(!out.stdout.is_empty());
    }
    let (out, v) = json_output(&root, &["env", "stop", "--json"], None);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(v["error"]["code"], "INVALID_INPUT");
}

#[test]
fn refresh_is_explicit_profile_scoped_and_supports_envelopes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.json");
    let catalog = json!({"format_version":1,"platform_version":"test","source":"fixture","methods":[{"namespace":"future","service":"service","name":"read","path":"future/service/rest/read","public":false,"doc_url":null,"parameters":[]}]});
    std::fs::write(&path, json!({"ok":true,"data":catalog}).to_string()).unwrap();
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "schema",
            "refresh",
            "--file",
            path.to_str().unwrap(),
            "--platform-version",
            "test",
            "--profile",
            "sandbox",
            "--json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(v["ok"], false);
    assert!(!root.path().join("catalogs").exists());
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "schema",
            "refresh",
            "--file",
            path.to_str().unwrap(),
            "--platform-version",
            "test",
            "--profile",
            "sandbox",
            "--yes",
            "--json",
        ],
        None,
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["added"], json!(["future/service/rest/read"]));
    let (out, v) = json_output(
        &root,
        &[
            "api",
            "describe",
            "future.service.read",
            "--profile",
            "sandbox",
            "--json",
        ],
        None,
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["platform_version"], "test");
    let (out, v) = json_output(
        &root,
        &["api", "search", "s3", "--profile", "default", "--json"],
        None,
    );
    assert!(out.status.success());
    assert_eq!(v["meta"]["catalog"]["methods"], 1376);
}

#[test]
fn explicit_targets_override_input_and_unsupported_targets_fail() {
    let root = tempfile::tempdir().unwrap();
    let (out, v) = json_output(
        &root,
        &[
            "env",
            "info",
            "--env",
            "explicit",
            "--input",
            "-",
            "--preview",
            "--json",
        ],
        Some(r#"{"envName":"from-file"}"#),
    );
    assert!(out.status.success(), "{v}");
    assert_eq!(v["data"]["parameters"]["envName"], "explicit");
    let (out, _) = json_output(
        &root,
        &["s3", "list", "--node", "123", "--preview", "--json"],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn partial_catalog_returns_json_for_every_missing_convenience_method() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = root.path().join("catalogs");
    std::fs::create_dir(&catalogs).unwrap();
    let catalog = json!({"format_version":1,"platform_version":"fixture","source":"fixture","methods":[{"namespace":"future","service":"service","name":"read","path":"future/service/rest/read","public":false,"doc_url":null,"parameters":[]}]});
    std::fs::write(catalogs.join("default.json"), catalog.to_string()).unwrap();
    for command in [
        "exec",
        "env list",
        "env info",
        "env create",
        "env clone",
        "env start",
        "env stop",
        "env delete",
        "env templates",
        "env regions",
        "node list",
        "node restart",
        "node scale",
        "logs list",
        "logs read",
        "logs watch",
        "files list",
        "files read",
        "files write",
        "files upload",
        "files delete",
        "vars list",
        "vars set",
        "vars delete",
        "projects list",
        "s3 list",
        "s3 create",
        "s3 delete",
        "s3 objects",
        "s3 create-account",
        "s3 keys",
        "s3 generate-key",
        "billing quotas",
        "billing pricing",
        "operations list",
        "operations get",
    ] {
        let mut args: Vec<_> = command.split_whitespace().collect();
        args.extend(["--preview", "--json"]);
        let (out, value) = json_output(&root, &args, None);
        assert_eq!(out.status.code(), Some(1), "{command}: {value}");
        assert_eq!(value["error"]["code"], "METHOD_UNAVAILABLE", "{command}");
        assert!(out.stderr.is_empty(), "{command}");
    }
}

#[test]
fn business_tokens_are_private_inputs_not_reserved_sessions() {
    let root = tempfile::tempdir().unwrap();
    for service in ["authentication", "sso"] {
        let input = if service == "authentication" {
            r#"{"token":"BUSINESS_SECRET_SENTINEL","userHeaders":"{}"}"#
        } else {
            r#"{"token":"BUSINESS_SECRET_SENTINEL"}"#
        };
        let (out, value) = json_output(
            &root,
            &[
                "api",
                "users",
                service,
                "signinbytoken",
                "--input",
                "-",
                "--preview",
                "--json",
            ],
            Some(input),
        );
        assert!(out.status.success(), "{value}");
        assert_eq!(value["data"]["parameters"]["token"], "[REDACTED]");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("BUSINESS_SECRET_SENTINEL"));
        for option in ["--param", "--token"] {
            let value = if option == "--param" {
                "token=BUSINESS_SECRET_SENTINEL"
            } else {
                "BUSINESS_SECRET_SENTINEL"
            };
            let (out, _) = json_output(
                &root,
                &[
                    "api",
                    "users",
                    service,
                    "signinbytoken",
                    option,
                    value,
                    "--preview",
                    "--json",
                ],
                None,
            );
            assert_eq!(out.status.code(), Some(2));
            assert!(!String::from_utf8_lossy(&out.stdout).contains("BUSINESS_SECRET_SENTINEL"));
        }
    }
    let (out, value) = json_output(
        &root,
        &[
            "env",
            "stop",
            "--env",
            "example",
            "--read-only",
            "--yes",
            "--json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(value["error"]["code"], "PERMISSION_DENIED");
}

#[test]
fn resolved_context_preserves_target_precedence_and_credential_origin() {
    let root = tempfile::tempdir().unwrap();
    let config = json!({"default_profile":"fixture","profiles":{"fixture":{"platform":"https://original.example","credential_store":"file","credential_kind":"session","credential_id":"missing-test-credential","imported_from":null,"environment":"saved","project":"673","node":"123"}}});
    std::fs::write(root.path().join("config.json"), config.to_string()).unwrap();
    for (extra, input, expected) in [
        (vec![], None, "saved"),
        (
            vec!["--input", "-"],
            Some(r#"{"domain":"from-file"}"#),
            "from-file",
        ),
        (
            vec!["--input", "-", "--env", "explicit"],
            Some(r#"{"envName":"from-file"}"#),
            "explicit",
        ),
    ] {
        let mut args = vec!["env", "info", "--preview", "--json"];
        args.extend(extra);
        let (out, value) = json_output(&root, &args, input);
        assert!(out.status.success(), "{value}");
        assert_eq!(value["data"]["parameters"]["envName"], expected);
    }
    let (out, value) = json_output(
        &root,
        &["s3", "list", "--project", "777", "--preview", "--json"],
        None,
    );
    assert!(out.status.success(), "{value}");
    assert_eq!(value["data"]["parameters"]["projectId"], 777);
    let (out, value) = json_output(
        &root,
        &[
            "env",
            "list",
            "--platform",
            "https://other.example",
            "--json",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(value["error"]["code"], "PERMISSION_DENIED");
}
