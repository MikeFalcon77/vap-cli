//! HTTP backend boundary. Published toolkit-http is used without local patches or FIPS.
use crate::error::{Error, Result};
use std::{path::Path, time::Duration};

#[cfg(feature = "reqwest-client")]
pub const NAME: &str = "reqwest";
#[cfg(feature = "toolkit-http-client")]
pub const NAME: &str = "toolkit-http 0.8.0";
#[cfg(feature = "reqwest-client")]
pub const TLS: &str = "rustls/ring";
#[cfg(feature = "toolkit-http-client")]
pub const TLS: &str = "rustls/AWS-LC (non-FIPS)";

pub struct Failure {
    timeout: bool,
    connect: bool,
    body_limit: bool,
}
impl Failure {
    pub fn is_timeout(&self) -> bool {
        self.timeout
    }
    pub fn is_connect(&self) -> bool {
        self.connect
    }
    pub fn is_body_limit(&self) -> bool {
        self.body_limit
    }
}
#[cfg(feature = "reqwest-client")]
impl From<reqwest::Error> for Failure {
    fn from(e: reqwest::Error) -> Self {
        Self {
            timeout: e.is_timeout(),
            connect: e.is_connect(),
            body_limit: false,
        }
    }
}
#[cfg(feature = "toolkit-http-client")]
impl From<toolkit_http::HttpError> for Failure {
    fn from(e: toolkit_http::HttpError) -> Self {
        Self {
            timeout: matches!(
                e,
                toolkit_http::HttpError::Timeout(_) | toolkit_http::HttpError::DeadlineExceeded(_)
            ),
            connect: matches!(e, toolkit_http::HttpError::Transport(_)),
            body_limit: matches!(e, toolkit_http::HttpError::BodyTooLarge { .. }),
        }
    }
}

pub struct Client {
    #[cfg(feature = "reqwest-client")]
    inner: reqwest::Client,
    #[cfg(feature = "toolkit-http-client")]
    inner: toolkit_http::HttpClient,
}

impl Client {
    pub fn new(platform: &str, timeout: Duration, ca: Option<&Path>) -> Result<Self> {
        #[cfg(feature = "reqwest-client")]
        {
            let _ = platform;
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            for cert in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(cert);
            }
            if let Some(ca) = ca {
                use rustls::pki_types::pem::PemObject;
                let bytes = std::fs::read(ca)?;
                let mut count = 0;
                for cert in rustls::pki_types::CertificateDer::pem_slice_iter(&bytes) {
                    roots
                        .add(cert.map_err(|_| Error::new("TLS_ERROR", "Invalid CA PEM"))?)
                        .map_err(|_| Error::new("TLS_ERROR", "Invalid CA certificate"))?;
                    count += 1;
                }
                if count == 0 {
                    return Err(Error::new(
                        "TLS_ERROR",
                        "CA file contains no PEM certificates",
                    ));
                }
            }
            let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::new("TLS_ERROR", "Cannot initialize TLS"))?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let builder = reqwest::Client::builder()
                .tls_backend_preconfigured(tls)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .timeout(timeout)
                .user_agent(concat!("vap-cli/", env!("CARGO_PKG_VERSION")));
            #[cfg(test)]
            let builder = builder.no_proxy();
            Ok(Self {
                inner: builder
                    .build()
                    .map_err(|_| Error::new("NETWORK_ERROR", "Cannot initialize HTTP client"))?,
            })
        }
        #[cfg(feature = "toolkit-http-client")]
        {
            if ca.is_some() {
                return Err(Error::new(
                    "UNSUPPORTED_FEATURE",
                    "Published toolkit-http 0.8.0 has no custom CA injection on HttpClientBuilder",
                )
                .hint(
                    "Use the reqwest-client variant for --ca; TLS verification is never disabled",
                ));
            }
            if [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
            {
                return Err(Error::new(
                    "UNSUPPORTED_FEATURE",
                    "Published toolkit-http 0.8.0 has no proxy connector configuration",
                )
                .hint("Use the reqwest-client variant in environments requiring a proxy"));
            }
            let mut cfg = toolkit_http::HttpClientConfig::minimal();
            cfg.request_timeout = timeout;
            cfg.total_timeout = Some(timeout);
            cfg.retry = None;
            cfg.rate_limit = None;
            cfg.max_body_size = max_response_bytes();
            cfg.redirect = toolkit_http::RedirectConfig::disabled();
            cfg.otel = false;
            cfg.tls_roots = toolkit_http::TlsRootConfig::Native;
            cfg.transport = toolkit_http::TransportSecurity::TlsOnly;
            #[cfg(test)]
            if platform.starts_with("http://127.0.0.1:") {
                cfg.transport = toolkit_http::TransportSecurity::AllowInsecureHttp;
            }
            let _ = platform;
            cfg.user_agent = concat!("vap-cli/", env!("CARGO_PKG_VERSION")).into();
            let inner = toolkit_http::HttpClientBuilder::with_config(cfg)
                .build()
                .map_err(|_| {
                    Error::new("NETWORK_ERROR", "Cannot initialize toolkit-http client")
                })?;
            Ok(Self { inner })
        }
    }
    pub async fn post_form(
        &self,
        url: &str,
        form: &[(String, String)],
    ) -> std::result::Result<Response, Failure> {
        #[cfg(feature = "reqwest-client")]
        {
            self.inner
                .post(url)
                .form(form)
                .send()
                .await
                .map(|inner| Response {
                    status: inner.status(),
                    headers: inner.headers().clone(),
                    inner,
                })
                .map_err(Failure::from)
        }
        #[cfg(feature = "toolkit-http-client")]
        {
            let fields: Vec<_> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let inner = self
                .inner
                .post(url)
                .form(&fields)
                .map_err(Failure::from)?
                .send()
                .await
                .map_err(Failure::from)?;
            Ok(Response {
                status: inner.status(),
                headers: inner.headers().clone(),
                inner: inner.into_limited_body(),
            })
        }
    }
}

pub struct Response {
    status: http::StatusCode,
    headers: http::HeaderMap,
    #[cfg(feature = "reqwest-client")]
    inner: reqwest::Response,
    #[cfg(feature = "toolkit-http-client")]
    inner: toolkit_http::LimitedBody,
}
impl Response {
    pub fn status(&self) -> http::StatusCode {
        self.status
    }
    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }
    pub async fn chunk(&mut self) -> std::result::Result<Option<bytes::Bytes>, Failure> {
        #[cfg(feature = "reqwest-client")]
        {
            self.inner.chunk().await.map_err(Failure::from)
        }
        #[cfg(feature = "toolkit-http-client")]
        {
            use http_body_util::BodyExt;
            loop {
                match self.inner.frame().await {
                    Some(Ok(frame)) => {
                        if let Ok(data) = frame.into_data() {
                            return Ok(Some(data));
                        }
                    }
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(None),
                }
            }
        }
    }
}

pub fn max_response_bytes() -> usize {
    std::env::var("VAP_MAX_RESPONSE_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64 * 1024 * 1024)
}
