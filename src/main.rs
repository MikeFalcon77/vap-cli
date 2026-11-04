mod app;
mod auth;
mod catalog;
mod cli;
mod config;
mod error;
mod http_client;
mod output;
mod progress;
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
