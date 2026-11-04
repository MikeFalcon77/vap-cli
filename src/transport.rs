use crate::{
    error::{Error, Result},
    request::PreparedRequest,
};
#[cfg(test)]
use serde_json::Map;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub struct Client {
    http: crate::http_client::Client,
    base: String,
    timeout: Duration,
}

impl Client {
    pub fn new(platform: &str, timeout: Duration, ca: Option<&Path>) -> Result<Self> {
        let http = crate::http_client::Client::new(platform, timeout, ca)?;
        Ok(Self {
            http,
            base: platform.into(),
            timeout,
        })
    }

    pub async fn call(
        &self,
        request: &PreparedRequest,
        token: Option<&str>,
        deadline: tokio::time::Instant,
    ) -> Result<Value> {
        let read_only = request.policy().read_only();
        let timeout = || {
            Error::new(
                if read_only {
                    "TIMEOUT"
                } else {
                    "OUTCOME_UNKNOWN"
                },
                "Request deadline reached; a server operation may still continue",
            )
            .hint("Inspect current operations and resource state before retrying")
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(timeout());
        }
        tokio::time::timeout_at(deadline, self.call_inner(request, token))
            .await
            .map_err(|_| timeout())?
    }

    async fn call_inner(&self, request: &PreparedRequest, token: Option<&str>) -> Result<Value> {
        let path = request.path();
        let params = request.params();
        let read_only = request.policy().read_only();
        let mut form: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        Value::String(s) => s.clone(),
                        _ => v.to_string(),
                    },
                )
            })
            .collect();
        if let Some(token) = token {
            form.retain(|(k, _)| k != "session");
            form.push(("session".into(), token.into()));
        }
        let mut attempts = 0;
        loop {
            let response = self
                .http
                .post_form(&format!("{}/1.0/{path}", self.base), &form)
                .await;
            let mut response = match response {
                Ok(r) => r,
                Err(e) => {
                    if read_only && attempts < 2 && (e.is_timeout() || e.is_connect()) {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(250 * attempts)).await;
                        continue;
                    }
                    return Err(if e.is_timeout() {
                        Error::new(
                            if read_only {
                                "TIMEOUT"
                            } else {
                                "OUTCOME_UNKNOWN"
                            },
                            if read_only {
                                "Request timed out"
                            } else {
                                "Request timed out; the server may still complete the operation"
                            },
                        )
                        .hint("Inspect current operations and resource state before retrying")
                    } else {
                        Error::new(if read_only {"NETWORK_ERROR"}else{"OUTCOME_UNKNOWN"},"Connection failed; a modifying request may have reached the server").hint("Check platform/proxy/CA settings and inspect resource state before retrying")
                    });
                }
            };
            let status = response.status();
            if read_only && attempts < 2 && (status.as_u16() == 429 || status.as_u16() == 503) {
                let delay = retry_delay(
                    response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                    attempts,
                );
                attempts += 1;
                tokio::time::sleep(delay.min(self.timeout)).await;
                continue;
            }
            if !status.is_success() {
                return Err(Error::new(match status.as_u16(){401=>"SESSION_EXPIRED",403=>"PERMISSION_DENIED",404=>"METHOD_UNAVAILABLE",429=>"RATE_LIMITED",_=>"HTTP_ERROR"},format!("Platform returned HTTP {}",status.as_u16())).hint(match status.as_u16(){401=>"Reauthenticate or import a fresh session; imported sessions cannot refresh automatically",403=>"Check account role and token permissions",404=>"Check platform version and vap api describe",_=>"Inspect platform status; modifying requests are never retried automatically"}));
            }
            // Bound decompressed response memory. Large responses remain possible with an explicit setting.
            let limit = crate::http_client::max_response_bytes();
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| {
                if e.is_body_limit(){return Error::new("RESPONSE_TOO_LARGE","Response exceeds VAP_MAX_RESPONSE_BYTES; operation may already have completed");}
                Error::new(
                    if read_only {
                        "NETWORK_ERROR"
                    } else {
                        "OUTCOME_UNKNOWN"
                    },
                    "Response was interrupted; inspect state before retrying",
                )
            })? {
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(Error::new(
                        "RESPONSE_TOO_LARGE",
                        "Response exceeds VAP_MAX_RESPONSE_BYTES; operation may already have completed",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            let value:Value=serde_json::from_slice(&bytes).map_err(|_|Error::new("INVALID_RESPONSE","Platform returned invalid JSON; a modifying operation may already have completed"))?;
            check_result(&value)?;
            return Ok(value);
        }
    }
}

fn retry_delay(value: Option<&str>, attempt: u64) -> Duration {
    value
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_millis(500 * (attempt + 1)))
}

pub fn check_result(value: &Value) -> Result<()> {
    let mut current = value;
    loop {
        let code = current
            .get("result")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .ok_or_else(|| {
                Error::new(
                    "INVALID_RESPONSE",
                    "API response has no numeric result; success cannot be established",
                )
            })?;
        if code != 0 {
            let detail = current
                .get("error")
                .or_else(|| current.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let kind = if detail.contains("session")
                && (detail.contains("expired") || detail.contains("invalid"))
            {
                "SESSION_EXPIRED"
            } else if detail.contains("permission") || detail.contains("access denied") {
                "PERMISSION_DENIED"
            } else if detail.contains("method")
                && (detail.contains("not found") || detail.contains("not supported"))
            {
                "METHOD_UNAVAILABLE"
            } else {
                "API_ERROR"
            };
            let mut error = Error::new(
                kind,
                format!("Platform operation failed (API result {code})"),
            );
            error.api_code = Some(code);
            error.hint = Some(
                if code == 8400 {
                    "Specify --project with the engineProjectId from vap projects list"
                } else {
                    "Inspect method requirements, account permissions and current operation state"
                }
                .into(),
            );
            return Err(error);
        }
        for key in ["responses", "bucketsList"] {
            if let Some(children) = current.get(key).and_then(Value::as_array) {
                for child in children.iter().filter(|v| v.get("result").is_some()) {
                    check_result(child)?;
                }
            }
        }
        if let Some(child) = current
            .get("response")
            .filter(|v| v.is_object() && v.get("result").is_some())
        {
            current = child;
        } else {
            break;
        }
    }
    Ok(())
}

pub fn payload(mut value: Value) -> Value {
    while value
        .get("response")
        .is_some_and(|v| v.is_object() && v.get("result").is_some())
    {
        value = value["response"].take();
    }
    value
}

pub fn preview(request: &PreparedRequest, platform: Option<&str>) -> Value {
    json!({"preview":true,"validation":"local request construction only; no server request or cost guarantee","platform":platform,"http_method":"POST","path":format!("/1.0/{}",request.path()),"parameters":request.params(),"credential":"[REDACTED]","read_only":request.policy().read_only()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_batch_failures_are_not_success() {
        for value in [
            json!({"result":0,"responses":[{"result":7}]}),
            json!({"result":0,"bucketsList":[{"result":8}]}),
        ] {
            assert!(check_result(&value).is_err());
        }
        assert!(
            check_result(
                &json!({"result":0,"responses":[{"name":"Default","engineProjectId":123}]})
            )
            .is_ok()
        );
    }
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    fn server(
        responses: Vec<(u16, String)>,
        delay: Duration,
    ) -> (
        String,
        Arc<AtomicUsize>,
        std::thread::JoinHandle<Vec<String>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let handle = std::thread::spawn(move || {
            let mut requests = vec![];
            for (status, body) in responses {
                let mut stream = crate::test_support::accept(&listener);
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = vec![];
                let mut buf = [0; 8192];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(split) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..split]);
                        let size = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= split + 4 + size {
                            break;
                        }
                    }
                }
                calls.fetch_add(1, Ordering::SeqCst);
                requests.push(String::from_utf8(bytes).unwrap());
                std::thread::sleep(delay);
                let redirect = if status == 302 {
                    "Location: /redirected\r\n"
                } else {
                    ""
                };
                let response = format!(
                    "HTTP/1.1 {status} Test\r\n{redirect}Content-Type: application/json\r\nRetry-After: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
            requests
        });
        (base, count, handle)
    }
    #[tokio::test]
    async fn redirects_are_not_followed_by_either_backend() {
        let (base, count, server) = server(vec![(302, "{}".into())], Duration::ZERO);
        let client = Client::new(&base, Duration::from_secs(5), None).unwrap();
        assert_eq!(
            client
                .call(
                    &PreparedRequest::new("environment/control/rest/getenvs", Map::new().clone())
                        .unwrap(),
                    Some("fake"),
                    tokio::time::Instant::now() + client.timeout
                )
                .await
                .unwrap_err()
                .code,
            "HTTP_ERROR"
        );
        server.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn form_encoding_and_nested_api_error() {
        let (base, _, server) = server(
            vec![(200, r#"{"result":0,"response":{"result":2395}}"#.into())],
            Duration::ZERO,
        );
        let client = Client::new(&base, Duration::from_secs(5), None).unwrap();
        let params =
            json!({"env":{"shortdomain":"тест"},"nodes":[{"diskLimit":100}],"enabled":false})
                .as_object()
                .unwrap()
                .clone();
        let e = client
            .call(
                &PreparedRequest::new("environment/control/rest/createenvironment", params.clone())
                    .unwrap(),
                Some("FAKE_SESSION"),
                tokio::time::Instant::now() + client.timeout,
            )
            .await
            .unwrap_err();
        assert_eq!(e.api_code, Some(2395));
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(!headers.contains("FAKE_SESSION"));
        assert!(body.contains("session=FAKE_SESSION"));
        assert!(body.contains("diskLimit"));
        assert!(body.contains("%D1%82"));
        assert!(body.contains("enabled=false"));
    }
    #[tokio::test]
    async fn retries_only_audited_reads() {
        let (base, count, server) = server(
            vec![
                (503, "{}".into()),
                (429, "{}".into()),
                (200, r#"{"result":0}"#.into()),
            ],
            Duration::ZERO,
        );
        let client = Client::new(&base, Duration::from_secs(5), None).unwrap();
        client
            .call(
                &PreparedRequest::new("environment/control/rest/getenvs", Map::new().clone())
                    .unwrap(),
                Some("fake"),
                tokio::time::Instant::now() + client.timeout,
            )
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
        let (base, count, server) = self::server(vec![(503, "{}".into())], Duration::ZERO);
        let client = Client::new(&base, Duration::from_secs(5), None).unwrap();
        assert_eq!(
            client
                .call(
                    &PreparedRequest::new("environment/control/rest/stopenv", Map::new().clone())
                        .unwrap(),
                    Some("fake"),
                    tokio::time::Instant::now() + client.timeout
                )
                .await
                .unwrap_err()
                .code,
            "HTTP_ERROR"
        );
        server.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn modification_timeout_is_unknown_and_not_retried() {
        let (base, count, server) = server(
            vec![(200, r#"{"result":0}"#.into())],
            Duration::from_millis(300),
        );
        let client = Client::new(&base, Duration::from_millis(100), None).unwrap();
        assert_eq!(
            client
                .call(
                    &PreparedRequest::new(
                        "environment/control/rest/createenvironment",
                        Map::new().clone()
                    )
                    .unwrap(),
                    Some("fake"),
                    tokio::time::Instant::now() + client.timeout
                )
                .await
                .unwrap_err()
                .code,
            "OUTCOME_UNKNOWN"
        );
        server.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn catalog_can_exceed_old_java_heap() {
        let body = format!(
            "{{\"result\":0,\"templates\":\"{}\"}}",
            "x".repeat(33 * 1024 * 1024)
        );
        let (base, _, server) = server(vec![(200, body)], Duration::ZERO);
        let client = Client::new(&base, Duration::from_secs(15), None).unwrap();
        let value = client
            .call(
                &PreparedRequest::new("environment/control/rest/gettemplates", Map::new().clone())
                    .unwrap(),
                Some("fake"),
                tokio::time::Instant::now() + client.timeout,
            )
            .await
            .unwrap();
        assert_eq!(value["templates"].as_str().unwrap().len(), 33 * 1024 * 1024);
        server.join().unwrap();
    }
    #[test]
    fn nested_errors() {
        let e=check_result(&json!({"result":0,"response":{"result":0,"response":{"result":2395,"error":"echoed secret"}}})).unwrap_err();
        assert_eq!(e.api_code, Some(2395));
        assert!(!e.message.contains("echoed"));
    }
    #[test]
    fn does_not_interpret_arbitrary_user_data() {
        assert!(check_result(&json!({"result":0,"data":{"result":5}})).is_ok());
    }
    #[test]
    fn unwraps_transport_only() {
        assert_eq!(
            payload(json!({"result":0,"response":{"result":0,"data":[1]}})),
            json!({"result":0,"data":[1]})
        );
    }
}
