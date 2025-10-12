use std::{ffi::OsString, path::PathBuf, str::FromStr, sync::Arc, time::Duration};

use color_eyre::eyre::Context;
use envconfig::Envconfig;
use jiff::Timestamp;
use mail_send::Credentials;
use opentelemetry::{KeyValue, global};
use reqwest::redirect::Policy;
use tokio::signal::unix::SignalKind;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    actor::{email::email_sender, playlist::playlist_updater, web::web_server},
    database::Database,
    oauth::TokenManager,
};

//  mod playlist;
mod actor;
mod database;
mod oauth;

#[derive(Envconfig)]
pub struct Config {
    #[envconfig(from = "GOOGLE_CLIENT_ID")]
    pub google_client_id: String,
    #[envconfig(from = "GOOGLE_CLIENT_SECRET")]
    pub google_client_secret: String,

    #[envconfig(from = "ALERTS_SMTP_USERNAME")]
    pub alerts_smtp_username: String,
    #[envconfig(from = "ALERTS_SMTP_PASSWORD")]
    pub alerts_smtp_password: String,

    #[envconfig(from = "YOUTUBE_PLAYLIST_ID")]
    pub youtube_playlist_id: String,

    #[envconfig(from = "HOSTNAME")]
    pub hostname: String,

    #[envconfig(from = "EARLIEST_UPDATE")]
    pub earliest_update: String,

    #[envconfig(from = "DATABASE_URL")]
    pub database_url: OsString,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Telemetry
    global::set_tracer_provider(
        opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(
                opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .build()?,
            )
            .build(),
    );
    global::set_meter_provider(
        opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(
                opentelemetry_otlp::MetricExporter::builder()
                    .with_tonic()
                    .build()?,
            )
            .build(),
    );

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_file(true)
                .with_line_number(true),
        )
        .with(tracing_opentelemetry::layer().with_tracer(global::tracer("tracing")))
        .with(ErrorLayer::default())
        .with(EnvFilter::from_default_env())
        .init();

    // tracing::trace!("a");
    // tracing::debug!("a");
    // tracing::info!("a");
    // tracing::warn!("a");
    // tracing::error!("a");

    let config = Config::init_from_env().unwrap();

    let google_client_id = oauth2::ClientId::new(config.google_client_id);
    let google_client_secret = oauth2::ClientSecret::new(config.google_client_secret);

    let email_credentials =
        Credentials::new(config.alerts_smtp_username, config.alerts_smtp_password);

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

    let database = Database::create(
        PathBuf::from(config.database_url),
        Timestamp::from_str(&config.earliest_update)?,
    )
    .await
    .wrap_err("unable to open database file")?;

    let token_manager = TokenManager::init(
        database.clone(),
        google_client_id,
        google_client_secret,
        config.hostname.clone(),
        email_send_tx,
    )
    .await
    .wrap_err("unable to initialize the token manager")?;

    let meter = global::meter("youtube-scraper");
    let scrapes_counter = meter
        .u64_counter("scrapes_total")
        .with_description("Total number of scrapes performed.")
        .build();
    let scrapes_counter = meter
        .u64_counter("videos_added_total")
        .with_description("Total number of videos added.")
        .build();
    let error_counter = meter
        .u64_counter("errors_total")
        .with_description("Total number of errors encountered.")
        .build();
    let scrape_duration_meter = meter
        .f64_gauge("last_scrape_duration_seconds")
        .with_description("Duration of previous scrape.")
        .build();

    // error_counter.add(1, &[KeyValue::new("a", "b")]);

    // error_counter.add(1, &[KeyValue::new("error.type", error_type)]);

    let shutdown = CancellationToken::new();

    let tasks = TaskTracker::new();

    // Un-authenticated services
    let mut web_server_task = tasks.spawn(web_server(shutdown.clone(), token_manager.clone()));

    // Oauth service
    let mut email_task = tasks.spawn(email_sender(
        shutdown.clone(),
        email_credentials,
        email_send_rx,
    ));

    // Authenticated services
    let mut subscription_task = tasks.spawn(playlist_updater(
        shutdown.clone(),
        database.clone(),
        client.clone(),
        token_manager,
        Arc::from(config.youtube_playlist_id),
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
