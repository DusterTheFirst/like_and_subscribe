use std::{sync::Arc, time::Duration};

use color_eyre::eyre::Context;
use mail_send::Credentials;
use migration::{Migrator, MigratorTrait as _};
use reqwest::redirect::Policy;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tokio::{signal::unix::SignalKind, sync::Notify};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    actor::{
        email::email_sender,
        pubsubhubbub::{queue::pubsub_queue_consumer, refresh::pubsub_refresh},
        subscription::subscription_manager,
        web::web_server,
    },
    oauth::TokenManager,
};

//  mod playlist;
mod actor;
mod database;
mod feed;
mod oauth;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_file(true)
                .with_line_number(true),
        )
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

    let google_client_id = oauth2::ClientId::new(
        std::env::var("GOOGLE_CLIENT_ID").wrap_err("unable to read GOOGLE_CLIENT_ID env var")?,
    );
    let google_client_secret = oauth2::ClientSecret::new(
        std::env::var("GOOGLE_CLIENT_SECRET")
            .wrap_err("unable to read GOOGLE_CLIENT_SECRET env var")?,
    );

    let email_credentials = {
        Credentials::new(
            std::env::var("ALERTS_SMTP_USERNAME")
                .wrap_err("unable to read ALERTS_SMTP_USERNAME env var")?,
            std::env::var("ALERTS_SMTP_PASSWORD")
                .wrap_err("unable to read ALERTS_SMTP_PASSWORD env var")?,
        )
    };

    let playlist_id = std::env::var("YOUTUBE_PLAYLIST_ID")
        .wrap_err("Unable to read YOUTUBE_PLAYLIST_ID env var")?;

    let hostname = std::env::var("HOSTNAME").wrap_err("Unable to read HOSTNAME env var")?;

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

    let database: DatabaseConnection = Database::connect(ConnectOptions::new(
        std::env::var("DATABASE_URL").wrap_err("DATABASE_URL not set")?,
    ))
    .await
    .wrap_err("unable to open database file")?;

    // Apply all pending migrations
    Migrator::up(&database, None).await?;

    // TODO: some way to verify that the subscriptions are actually subscribed, maybe once a day?
    // https://pubsubhubbub.appspot.com/subscription-details?hub.callback=https%3A%2F%2Flenovo-fedora.taila5e2a.ts.net%2Fpubsub&hub.topic=https%3A%2F%2Fwww.youtube.com%2Fxml%2Ffeeds%2Fvideos.xml%3Fchannel_id%3DUCHtv-7yDeac7OSfPJA_a6aA&hub.secret=

    let pubsubhubbub_callback = format!("https://{hostname}/pubsub");

    let subscriptions_queue_notify = Arc::new(Notify::const_new());
    let video_queue_notify = Arc::new(Notify::const_new());

    let (email_send_tx, email_send_rx) = tokio::sync::mpsc::channel(1);

    let token_manager = TokenManager::init(
        database.clone(),
        google_client_id,
        google_client_secret,
        hostname.clone(),
        email_send_tx,
    )
    .await
    .wrap_err("unable to initialize the token manager")?;

    let shutdown = CancellationToken::new();

    let tasks = TaskTracker::new();

    // Unauthenticated services
    let mut web_server_task = tasks.spawn(web_server(
        shutdown.clone(),
        database.clone(),
        video_queue_notify.clone(),
        token_manager.clone(),
    ));
    let mut pubsubhubbub_queue_task = tasks.spawn(pubsub_queue_consumer(
        shutdown.clone(),
        database.clone(),
        subscriptions_queue_notify.clone(),
        client.clone(),
        pubsubhubbub_callback,
    ));
    let mut pubsubhubbub_refresh_task = tasks.spawn(pubsub_refresh(
        shutdown.clone(),
        database.clone(),
        subscriptions_queue_notify.clone(),
    ));

    // Oauth service
    // let mut oauth_task = tasks.spawn(async {});
    let mut email_task = tasks.spawn(email_sender(
        shutdown.clone(),
        email_credentials,
        email_send_rx,
    ));

    // Authenticated services
    let mut subscription_task = tasks.spawn(subscription_manager(
        shutdown.clone(),
        database.clone(),
        subscriptions_queue_notify.clone(),
        client.clone(),
        token_manager,
    ));
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
        result = &mut pubsubhubbub_queue_task => tracing::error!(?result, "pusubhubbub queue task exited"),
        result = &mut pubsubhubbub_refresh_task => tracing::error!(?result, "pubsubhubbub refresh task exited"),

        // result = &mut oauth_task => tracing::error!(?result, "oauth task exited"),
        result = &mut email_task => tracing::error!(?result, "email task exited"),

        result = &mut subscription_task => tracing::error!(?result, "subscription task exited"),
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
