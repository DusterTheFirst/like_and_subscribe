use std::{path::PathBuf, sync::Arc, time::Duration};

use color_eyre::eyre::{Context, eyre};
use mail_send::Credentials;
use reqwest::redirect::Policy;
use tokio::signal::unix::SignalKind;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    actor::{email::email_sender, subscription::subscription_manager, web::web_server},
    database::Database,
    oauth::TokenManager,
};

//  mod playlist;
mod actor;
mod database;
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

    let (email_send_tx, email_send_rx) = tokio::sync::mpsc::channel(1);

    let database = Database::create(PathBuf::from(
        std::env::var_os("DATABASE_URL").ok_or_else(|| eyre!("DATABASE_URL not set"))?,
    ))
    .await
    .wrap_err("unable to open database file")?;

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

    // Unauhenticated services
    let mut web_server_task = tasks.spawn(web_server(shutdown.clone(), token_manager.clone()));

    // Oauth service
    let mut email_task = tasks.spawn(email_sender(
        shutdown.clone(),
        email_credentials,
        email_send_rx,
    ));

    // Authenticated services
    let mut subscription_task = tasks.spawn(subscription_manager(
        shutdown.clone(),
        database.clone(),
        client.clone(),
        token_manager,
        Arc::from(playlist_id),
    ));

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

        result = &mut email_task => tracing::error!(?result, "email task exited"),

        result = &mut subscription_task => tracing::error!(?result, "subscription task exited"),

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
