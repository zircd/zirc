//! zirc's media worker: see the library's documentation.

use std::path::PathBuf;
use std::process::ExitStatus;

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
    /// imgproxy's program, run beside the worker for as long as it runs: a self-hosted
    /// server's media container (zirc-media) holds both. Unset, the worker runs alone.
    #[arg(long, env = "ZIRC_MEDIA_IMGPROXY")]
    imgproxy: Option<PathBuf>,
}

/// Waits until imgproxy stops; for ever when there's none.
async fn stopped(imgproxy: Option<&mut tokio::process::Child>) -> std::io::Result<ExitStatus> {
    match imgproxy {
        Some(child) => child.wait().await,
        None => std::future::pending().await,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    // reqwest's rustls comes without a crypto provider: ring, as zirc's other services use.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();
    let worker = zirc_media_worker::Worker::new(args.at_once, &args.scratch);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    // imgproxy reads its settings from the environment, as the image sets them.
    let mut imgproxy = match &args.imgproxy {
        Some(program) => Some(
            tokio::process::Command::new(program)
                .kill_on_drop(true)
                .spawn()?,
        ),
        None => None,
    };
    tracing::info!(listen = args.listen, "serving");
    let serve = axum::serve(listener, zirc_media_worker::server::router(worker))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        });
    // Without imgproxy the container is half of what it's for: it stops, and whatever
    // runs it starts it again.
    tokio::select! {
        served = async { serve.await } => Ok(served?),
        status = stopped(imgproxy.as_mut()) => anyhow::bail!("imgproxy stopped: {status:?}"),
    }
}
