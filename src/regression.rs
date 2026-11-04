//! Command parsing -> preparation -> real HTTP -> operation/output regression tests.
use crate::{
    catalog::Catalog,
    cli,
    engine::{Engine, ExecutionResult},
    error::Result,
    invocation::{Invocation, Selection},
    output,
    parameters::Targets,
    registry,
    request::Authentication,
    test_support::{Reply, Server},
    transport,
};
use serde_json::{Value, json};
use std::time::Duration;

fn prepare(args: &[&str], input: Value) -> Result<Invocation> {
    let catalog = Catalog::embedded()?;
    let matches = cli::build(&catalog)
        .try_get_matches_from(std::iter::once("vap").chain(args.iter().copied()))
        .unwrap();
    Selection::resolve(&matches, &catalog)?
        .prepare(&matches, Targets::default(), |_| Ok(input.to_string()))
}
async fn run(
    server: &Server,
    invocation: Invocation,
    timeout: Duration,
    interval: Duration,
    token: Option<&str>,
    events: &mut Vec<u8>,
) -> Result<Value> {
    let client = transport::Client::new(&server.url, timeout, None)?;
    let engine = Engine {
        client: &client,
        credential: token,
        timeout,
        interval,
    };
    let result = engine
        .run(
            &invocation.request,
            invocation.mode,
            |_| Ok(()),
            |event| output::write_success(&mut *events, event.data, event.meta, true),
        )
        .await?;
    Ok(match result {
        ExecutionResult::Response(value) => {
            registry::project(invocation.request.path(), invocation.spec, value)
        }
        ExecutionResult::StreamComplete => Value::Null,
    })
}

#[tokio::test]
async fn readlog_namespaces_have_independent_wire_parameters() {
    for count in [None, Some(17)] {
        let server = Server::new(vec![Reply::json(json!({"result":0}))]);
        let mut args = vec![
            "api",
            "environment",
            "control",
            "readlog",
            "--env",
            "example",
            "--node",
            "123",
            "--param",
            "path=/var/log/app.log",
        ];
        if count.is_some() {
            args.extend(["--param", "count=17"]);
        }
        let invocation = prepare(&args, json!({})).unwrap();
        run(
            &server,
            invocation,
            Duration::from_secs(5),
            Duration::ZERO,
            Some("FAKE_SESSION"),
            &mut vec![],
        )
        .await
        .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].line,
            "POST /1.0/environment/control/rest/readlog HTTP/1.1"
        );
        assert_eq!(requests[0].form["count"], count.unwrap_or(100).to_string());
    }
    let server = Server::new(vec![Reply::json(json!({"result":0}))]);
    let invocation = prepare(&["api", "marketplace", "console", "readlog"], json!({})).unwrap();
    run(
        &server,
        invocation,
        Duration::from_secs(5),
        Duration::ZERO,
        Some("FAKE_SESSION"),
        &mut vec![],
    )
    .await
    .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].line,
        "POST /1.0/marketplace/console/rest/readlog HTTP/1.1"
    );
    assert!(!requests[0].form.contains_key("count"));
}

#[tokio::test]
async fn signin_business_tokens_are_not_session_credentials() {
    for service in ["authentication", "sso"] {
        let server = Server::new(vec![Reply::json(
            json!({"result":0,"token":"BUSINESS_SECRET","session":"RETURNED_SESSION","echo":"BUSINESS_SECRET"}),
        )]);
        let input = if service == "authentication" {
            json!({"token":"BUSINESS_SECRET","userHeaders":"{}"})
        } else {
            json!({"token":"BUSINESS_SECRET"})
        };
        let invocation = prepare(
            &["api", "users", service, "signinbytoken", "--input", "-"],
            input,
        )
        .unwrap();
        assert!(
            invocation.authentication
                == if service == "sso" {
                    Authentication::Optional
                } else {
                    Authentication::Required
                }
        );
        let secrets =
            output::secret_values(&Value::Object(invocation.request.params().clone()), true);
        let mut preview = transport::preview(&invocation.request, None);
        output::redact(&mut preview, &secrets, true);
        assert!(!preview.to_string().contains("BUSINESS_SECRET"));
        let mut data = run(
            &server,
            invocation,
            Duration::from_secs(5),
            Duration::ZERO,
            Some("CURRENT_SESSION"),
            &mut vec![],
        )
        .await
        .unwrap();
        output::redact(&mut data, &secrets, true);
        assert!(!data.to_string().contains("BUSINESS_SECRET"));
        assert!(!data.to_string().contains("RETURNED_SESSION"));
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].form["token"], "BUSINESS_SECRET");
        assert_eq!(requests[0].form["session"], "CURRENT_SESSION");
        assert!(
            prepare(
                &[
                    "api",
                    "users",
                    service,
                    "signinbytoken",
                    "--param",
                    "token=BUSINESS_SECRET"
                ],
                json!({})
            )
            .is_err()
        );
    }
    let invocation = prepare(
        &["api", "users", "sso", "signinbytoken", "--input", "-"],
        json!({"token":"BUSINESS_SECRET"}),
    )
    .unwrap();
    let server = Server::new(vec![Reply::json(json!({"result":0}))]);
    run(
        &server,
        invocation,
        Duration::from_secs(5),
        Duration::ZERO,
        None,
        &mut vec![],
    )
    .await
    .unwrap();
    assert!(!server.requests()[0].form.contains_key("session"));
}

#[tokio::test]
async fn cli_policy_controls_permissions_and_actual_retries() {
    for args in [
        vec!["env", "stop", "--env", "example", "--read-only", "--yes"],
        vec![
            "api",
            "environment",
            "control",
            "stopenv",
            "--env",
            "example",
            "--read-only",
            "--yes",
        ],
        vec![
            "api",
            "call",
            "future/service/rest/read",
            "--read-only",
            "--yes",
        ],
    ] {
        let error = prepare(&args, json!({}))
            .err()
            .expect("Unsafe CLI operation must be rejected");
        assert_eq!(error.code, "PERMISSION_DENIED");
    }
    for args in [
        vec!["env", "stop", "--env", "example", "--yes"],
        vec!["api", "call", "future/service/rest/read", "--yes"],
    ] {
        let server = Server::new(vec![Reply::status(503), Reply::json(json!({"result":0}))]);
        let invocation = prepare(&args, json!({})).unwrap();
        let e = run(
            &server,
            invocation,
            Duration::from_secs(3),
            Duration::ZERO,
            Some("fake"),
            &mut vec![],
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "HTTP_ERROR");
        assert_eq!(server.requests().len(), 1);
    }
    let server = Server::new(vec![
        Reply::status(503),
        Reply::status(429),
        Reply::json(json!({"result":0,"infos":[]})),
    ]);
    let invocation = prepare(&["env", "list", "--read-only"], json!({})).unwrap();
    let value = run(
        &server,
        invocation,
        Duration::from_secs(3),
        Duration::ZERO,
        Some("fake"),
        &mut vec![],
    )
    .await
    .unwrap();
    assert_eq!(value, json!([]));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn watch_deadline_stops_sleep_and_prevents_second_request() {
    let server = Server::new(vec![
        Reply::json(json!({"result":0,"log":"one"})),
        Reply::json(json!({"result":0,"log":"two"})),
    ]);
    let invocation = prepare(
        &[
            "logs",
            "watch",
            "--env",
            "example",
            "--node",
            "123",
            "--param",
            "path=/tmp/log",
            "--timeout",
            "1",
            "--poll-interval",
            "10",
            "--samples",
            "2",
        ],
        json!({}),
    )
    .unwrap();
    let client = transport::Client::new(&server.url, Duration::from_secs(1), None).unwrap();
    let engine = Engine {
        client: &client,
        credential: Some("fake"),
        timeout: Duration::from_secs(1),
        interval: Duration::from_secs(10),
    };
    let started = std::time::Instant::now();
    let mut events = vec![];
    let e = engine
        .run(
            &invocation.request,
            invocation.mode,
            |_| Ok(()),
            |event| output::write_success(&mut events, event.data, event.meta, true),
        )
        .await
        .err()
        .expect("Watch must reach its deadline");
    let elapsed = started.elapsed();
    assert_eq!(e.code, "TIMEOUT");
    assert_eq!(server.requests().len(), 1);
    // TLS/client setup is outside the measurement. A 10s interval leaves a wide
    // scheduling margin while still detecting the old post-sleep deadline check.
    assert!(
        elapsed < Duration::from_secs(5),
        "Watch slept past its deadline: {elapsed:?}"
    );
    let event: Value = serde_json::from_slice(&events).unwrap();
    assert_eq!(event["schema_version"], 1);
    assert_eq!(event["meta"]["sample"], 1);
}

#[tokio::test]
async fn watch_deadline_interrupts_http_and_samples_produce_ndjson() {
    let mut delayed = Reply::json(json!({"result":0}));
    delayed.delay = Duration::from_millis(1200);
    let server = Server::new(vec![delayed]);
    let invocation = prepare(
        &[
            "logs",
            "watch",
            "--env",
            "example",
            "--node",
            "123",
            "--param",
            "path=/tmp/log",
        ],
        json!({}),
    )
    .unwrap();
    let mut events = vec![];
    let e = run(
        &server,
        invocation,
        Duration::from_millis(150),
        Duration::ZERO,
        Some("fake"),
        &mut events,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, "TIMEOUT");
    assert!(events.is_empty());
    assert_eq!(server.requests().len(), 1);
    let server = Server::new(vec![
        Reply::json(json!({"result":0,"log":"one"})),
        Reply::json(json!({"result":0,"log":"one"})),
        Reply::json(json!({"result":0,"log":"two"})),
    ]);
    let invocation = prepare(
        &[
            "logs",
            "watch",
            "--env",
            "example",
            "--node",
            "123",
            "--param",
            "path=/tmp/log",
            "--samples",
            "3",
        ],
        json!({}),
    )
    .unwrap();
    run(
        &server,
        invocation,
        Duration::from_secs(3),
        Duration::ZERO,
        Some("fake"),
        &mut events,
    )
    .await
    .unwrap();
    let lines: Vec<Value> = String::from_utf8(events)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["meta"]["sample"], 3);
    assert_eq!(server.requests().len(), 3);
    assert!(
        lines
            .iter()
            .all(|v| v["schema_version"] == 1 && v["ok"] == true)
    );
}

#[test]
fn response_views_do_not_cross_namespaces_and_new_commands_use_registry() {
    let catalog = Catalog::embedded().unwrap();
    let original = json!({"result":0,"infos":[{"env":{"envName":"one"}}]});
    assert_eq!(
        registry::project(
            "administration/cluster/rest/getenvs",
            None,
            original.clone()
        ),
        original
    );
    assert_eq!(
        registry::project("environment/control/rest/getenvs", None, original.clone()),
        original["infos"]
    );
    for spec in registry::COMMANDS {
        assert!(spec.method(&catalog).is_ok());
    }
}
