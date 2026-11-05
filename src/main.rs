mod app;
mod auth;
mod catalog;
mod cli;
mod config;
mod context;
mod engine;
mod error;
mod http_client;
mod invocation;
mod output;
mod parameters;
mod progress;
mod registry;
mod request;
mod transport;

#[cfg(any(
    all(feature = "reqwest-client", feature = "toolkit-http-client"),
    not(any(feature = "reqwest-client", feature = "toolkit-http-client"))
))]
compile_error!("Select exactly one HTTP backend: reqwest-client or toolkit-http-client");

#[tokio::main]
async fn main() {
    app::main().await;
}

#[cfg(test)]
mod regression;
#[cfg(test)]
mod test_support;
