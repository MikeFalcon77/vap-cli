#![cfg(feature = "reqwest-client")]
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

fn server(
    root: &tempfile::TempDir,
    response: &str,
) -> (String, std::path::PathBuf, std::thread::JoinHandle<String>) {
    server_sequence(root, &[response])
}
fn server_sequence(
    root: &tempfile::TempDir,
    responses: &[&str],
) -> (String, std::path::PathBuf, std::thread::JoinHandle<String>) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let ca = root.path().join("ca.pem");
    std::fs::write(&ca, cert.pem()).unwrap();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        signing_key.serialize_der(),
    ));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert.der().clone()], key)
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let responses: Vec<String> = responses.iter().map(|s| s.to_string()).collect();
    let thread = std::thread::spawn(move || {
        let mut requests = String::new();
        let config = Arc::new(config);
        for response in responses {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let conn = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, stream);
            let mut bytes = vec![];
            loop {
                let mut buf = [0; 8192];
                let n = tls.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(split) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..split]);
                    let length = headers
                        .lines()
                        .find_map(|s| {
                            s.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            write!(tls,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            tls.flush().unwrap();
            tls.conn.send_close_notify();
            let _ = tls.flush();
            requests.push_str(&String::from_utf8(bytes).unwrap());
        }
        requests
    });
    (url, ca, thread)
}
fn run(root: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vap"))
        .args(args)
        .env("VAP_CONFIG_DIR", root.path().join("config"))
        .env("VAP_TOKEN", "FAKE_SESSION_SENTINEL")
        .env_remove("VAP_PROFILE")
        .env_remove("VAP_PLATFORM")
        .env_remove("VAP_TOKEN_FILE")
        .env("NO_PROXY", "localhost,127.0.0.1")
        .env("no_proxy", "localhost,127.0.0.1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
#[test]
fn modifying_command_is_sent_once_and_one_time_key_is_saved_privately() {
    let root = tempfile::tempdir().unwrap();
    let (url, ca, thread) = server(
        &root,
        r#"{"result":0,"key":{"secKey":"ONE_TIME_TEST_KEY"}}"#,
    );
    let output = root.path().join("response.json");
    let out = run(
        &root,
        &[
            "env",
            "start",
            "--env",
            "example",
            "--yes",
            "--json",
            "--platform",
            &url,
            "--ca",
            ca.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    let request = thread.join().unwrap();
    assert!(
        out.status.success(),
        "{} / {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        request
            .matches("POST /1.0/environment/control/rest/startenv")
            .count(),
        1
    );
    assert!(request.contains("session=FAKE_SESSION_SENTINEL"));
    assert!(
        !request
            .split("\r\n\r\n")
            .next()
            .unwrap()
            .contains("FAKE_SESSION_SENTINEL")
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["data"]["key"]["secKey"], "[REDACTED]");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("ONE_TIME_TEST_KEY"));
    assert!(
        std::fs::read_to_string(&output)
            .unwrap()
            .contains("ONE_TIME_TEST_KEY")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn output_collision_fails_before_network() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("existing.json");
    std::fs::write(&path, "keep").unwrap();
    let out = run(
        &root,
        &[
            "env",
            "start",
            "--env",
            "example",
            "--yes",
            "--json",
            "--platform",
            "https://localhost:1",
            "--output",
            path.to_str().unwrap(),
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
}
#[test]
fn server_nested_failure_is_a_failed_process() {
    let root = tempfile::tempdir().unwrap();
    let (url, ca, thread) = server(
        &root,
        r#"{"result":0,"response":{"result":2395,"error":"FAKE_SESSION_SENTINEL"}}"#,
    );
    let out = run(
        &root,
        &[
            "env",
            "start",
            "--env",
            "example",
            "--yes",
            "--json",
            "--platform",
            &url,
            "--ca",
            ca.to_str().unwrap(),
        ],
    );
    thread.join().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["error"]["api_code"], 2395);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE_SESSION_SENTINEL"));
}

#[test]
fn wait_polls_readiness_without_repeating_the_change() {
    let root = tempfile::tempdir().unwrap();
    let (url, ca, thread) = server_sequence(
        &root,
        &[
            r#"{"result":0}"#,
            r#"{"result":0,"infos":[{"env":{"envName":"example","status":6},"nodes":[]}]}"#,
            r#"{"result":0,"infos":[{"env":{"envName":"example","status":1},"nodes":[{"id":123}]}]}"#,
        ],
    );
    let out = run(
        &root,
        &[
            "env",
            "start",
            "--env",
            "example",
            "--wait",
            "--poll-interval",
            "1",
            "--timeout",
            "5",
            "--yes",
            "--json",
            "--platform",
            &url,
            "--ca",
            ca.to_str().unwrap(),
        ],
    );
    let requests = thread.join().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        requests
            .matches("POST /1.0/environment/control/rest/startenv")
            .count(),
        1
    );
    assert_eq!(
        requests
            .matches("POST /1.0/environment/control/rest/getenvs")
            .count(),
        2
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["data"]["completed"], true);
}
