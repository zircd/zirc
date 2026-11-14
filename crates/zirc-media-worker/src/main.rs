//! zirc's media worker: see the library's documentation.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Probes zirc's videos and makes their posters and copies, as zirc.dev asks")]
struct Args {
    /// Where to listen.
    #[arg(long, env = "ZIRC_MEDIA_WORKER_LISTEN", default_value = "0.0.0.0:8080")]
    listen: String,
    /// How many posters and copies are made at once.
    #[arg(long, env = "ZIRC_MEDIA_WORKER_AT_ONCE", default_value_t = 2)]
    at_once: usize,
    /// Where what's being made is kept until it's put: an emptyDir.
    #[arg(long, env = "ZIRC_MEDIA_WORKER_SCRATCH", default_value = "/tmp")]
    scratch: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    // reqwest's rustls comes without a crypto provider: ring, as zirc's other services use.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();
    let worker = zirc_media_worker::Worker::new(args.at_once, &args.scratch);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = args.listen, "serving");
    axum::serve(listener, zirc_media_worker::server::router(worker))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
