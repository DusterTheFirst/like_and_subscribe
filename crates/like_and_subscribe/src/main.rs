use std::{fs::File, io::BufReader, sync::Arc, time::Duration};

use color_eyre::eyre::Context;
use migration::{Migrator, MigratorTrait as _};
use reqwest::redirect::Policy;
use sea_orm::{Database, DatabaseConnection};
use tokio::{signal::unix::SignalKind, sync::Notify};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{oauth::ApplicationSecretFile, web::web_server};

pub mod feed;
pub mod oauth;
// pub mod playlist;
// pub mod subscription;
pub mod web;
pub mod database;

// TODO: FIXME: better token refreshing (send an email or something)
// TODO: FIXME: local tailnet vs external tailnet URLs. Basically only pubsub should be external
// TODO: https://github.com/stalwartlabs/mail-send https://github.com/stalwartlabs/mail-builder

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_journald::layer()
                .wrap_err("tracing journald subscriber failed to initialize")?,
        )
        .with(ErrorLayer::default())
        .with(EnvFilter::from_default_env())
        .init();

    tracing::trace!("a");
    tracing::debug!("a");
    tracing::info!("a");
    tracing::warn!("a");
    tracing::error!("a");

    let oauth_secret_file = File::open(
        std::env::var("GOOGLE_CLIENT_SECRET_FILE")
            .wrap_err("Unable to read GOOGLE_CLIENT_SECRET_FILE env var")?,
    )
    .wrap_err("unable to open google client secret file")?;
    let oauth_secret_file: ApplicationSecretFile =
        serde_json::from_reader(BufReader::new(oauth_secret_file))
            .wrap_err("unable to parse google client secret file")?;
    let oauth_secret = oauth_secret_file.web;

    let playlist_id = std::env::var("YOUTUBE_PLAYLIST_ID")
        .wrap_err("Unable to read YOUTUBE_PLAYLIST_ID env var")?;

    let hostname =
        std::env::var("PUBSUB_HOSTNAME").wrap_err("Unable to read PUBSUB_HOSTNAME env var")?;

    // TODO: lettre notifications to fastmail w/ sorting to a special folder for problems
    let client = reqwest::ClientBuilder::new()
        .https_only(true)
        .connector_layer(
            ServiceBuilder::new()
                .concurrency_limit(10)
                .buffer(1024)
                .rate_limit(5, Duration::from_secs(10)), // TODO: does this mean 5 sets of 10?
        )
        .redirect(Policy::none())
        .build()
        .wrap_err("Unable to setup reqwest client")?;

    let database: DatabaseConnection = Database::connect("sqlite://database.sqlite?mode=rwc")
        .await
        .wrap_err("unable to open database file")?;

    // Apply all pending migrations
    Migrator::up(&database, None).await?;

    // TODO: https://docs.rs/google-apis-common/latest/google_apis_common/auth/index.html
    // TODO: https://developers.google.com/youtube/v3/guides/moving_to_oauth#using-oauth-2.0-for-server-side,-standalone-scripts
    // TODO: https://developers.google.com/youtube/v3/guides/moving_to_oauth#offline_access
    // TODO: Provide your own `AuthenticatorDelegate` to adjust the way it operates and get feedback about
    // what's going on. You probably want to bring in your own `TokenStorage` to persist tokens and
    // retrieve them from storage.

    // scopes: "https://www.googleapis.com/auth/youtube.readonly",            "https://www.googleapis.com/auth/youtube"

    // TODO: some way to verify that the subscriptions are actually subscribed, maybe once a day?
    // https://pubsubhubbub.appspot.com/subscription-details?hub.callback=https%3A%2F%2Flenovo-fedora.taila5e2a.ts.net%2Fpubsub&hub.topic=https%3A%2F%2Fwww.youtube.com%2Fxml%2Ffeeds%2Fvideos.xml%3Fchannel_id%3DUCHtv-7yDeac7OSfPJA_a6aA&hub.secret=

    let subscriptions_queue_notify = Arc::new(Notify::const_new());
    let video_queue_notify = Arc::new(Notify::const_new());

    let shutdown = CancellationToken::new();

    let tasks = TaskTracker::new();

    // Unauthenticated services
    let mut web_server_task =
        tasks.spawn(web_server(shutdown.clone(), database, video_queue_notify));
    // let mut pubsubhubbub_queue_task = tasks.spawn(async {});
    // let mut pubsubhubbub_refresh_task = tasks.spawn(async {});

    // Authenticated services
    // let mut subscription_task = tasks.spawn(async {});
    // let mut video_task = tasks.spawn(async {});

    // Shutdown signals
    let mut sigint_task = tokio::signal::unix::signal(SignalKind::interrupt()).unwrap();
    let mut sigquit_task = tokio::signal::unix::signal(SignalKind::quit()).unwrap();
    let mut sighup_task = tokio::signal::unix::signal(SignalKind::hangup()).unwrap();
    let mut sigterm_task = tokio::signal::unix::signal(SignalKind::terminate()).unwrap();

    let mut shutdown_signal = async move || {
        tokio::select! {
            Some(_) = sigint_task.recv() => {
                tracing::info!("Received signal INTERRUPT");
            },
            Some(_) = sigquit_task.recv() => {
                tracing::info!("Received signal QUIT");
            },
            Some(_) = sighup_task.recv() => {
                tracing::info!("Received signal HANGUP");
            },
            Some(_) = sigterm_task.recv() => {
                tracing::info!("Received signal TERMINATE");
            },
        }
    };

    // TODO: re-spawn failed tasks?
    tokio::select! {
        result = &mut web_server_task => tracing::error!(?result, "web server task exited"),
        // result = &mut pubsubhubbub_queue_task => tracing::error!(?result, "pusubhubbub queue task exited"),
        // result = &mut pubsubhubbub_refresh_task => tracing::error!(?result, "pubsubhubbub refresh task exited"),

        // result = &mut subscription_task => tracing::error!(?result, "subscription task exited"),
        // result = &mut video_task => tracing::error!(?result, "video task exited"),

        _ = shutdown_signal() => tracing::warn!("User requested exit"),
    }

    shutdown.cancel();
    tasks.close();

    tracing::info!("Performing clean shutdown");

    // Wait for clean shutdown, or next interrupt
    tokio::select! {
        () = tasks.wait() => tracing::info!("exited gracefully"),
        _ = shutdown_signal() => tracing::warn!("user sent second exit request during clean shutown"),
    }

    Ok(())
}
