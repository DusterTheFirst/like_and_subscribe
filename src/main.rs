use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    sync::{Arc, Mutex},
    time::Duration,
};

use color_eyre::eyre::{Context, eyre};
use google_youtube3::{
    YouTube,
    yup_oauth2::{self, ConsoleApplicationSecret},
};
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use reqwest::redirect::Policy;
use tower::ServiceBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    playlist::youtube_playlist_modifier,
    pubsub::youtube_pubsub_reciever,
    subscription::{YoutubeChannelSubscription, youtube_subscription_manager},
};

pub mod feed;
pub mod playlist;
pub mod pubsub;
pub mod subscription;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let opentelemetry_provider = SdkTracerProvider::builder()
        .with_batch_exporter(
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .build()
                .expect("otlp span exporter should be correctly configured"),
        )
        .build();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(opentelemetry_provider.tracer("tracing")))
        .with(ErrorLayer::default())
        .with(EnvFilter::from_default_env())
        .init();

    tracing::trace!("a");
    tracing::debug!("a");
    tracing::info!("a");
    tracing::warn!("a");
    tracing::error!("a");

    let google_client_secret: ConsoleApplicationSecret = serde_json::from_reader(BufReader::new(
        File::open(
            std::env::var("GOOGLE_CLIENT_SECRET_FILE")
                .wrap_err("Unable to read GOOGLE_CLIENT_SECRET_FILE env var")?,
        )
        .wrap_err("unable to open google client secret file")?,
    ))
    .wrap_err("unable to parse google client secret file")?;

    let playlist_id = std::env::var("YOUTUBE_PLAYLIST_ID")
        .wrap_err("Unable to read YOUTUBE_PLAYLIST_ID env var")?;

    let hostname =
        std::env::var("PUBSUB_HOSTNAME").wrap_err("Unable to read PUBSUB_HOSTNAME env var")?;

    // TODO: lettre notifications to fastmail w/ sorting to a special folder for problems
    // TODO: store logs in sql
    // TODO: store pubsubhubbub subscriptions in sql

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

    // TODO: https://docs.rs/google-apis-common/latest/google_apis_common/auth/index.html
    // TODO: https://developers.google.com/youtube/v3/guides/moving_to_oauth#using-oauth-2.0-for-server-side,-standalone-scripts
    // TODO: https://developers.google.com/youtube/v3/guides/moving_to_oauth#offline_access
    // TODO: Provide your own `AuthenticatorDelegate` to adjust the way it operates and get feedback about
    // what's going on. You probably want to bring in your own `TokenStorage` to persist tokens and
    // retrieve them from storage.
    let auth = yup_oauth2::InstalledFlowAuthenticator::builder(
        Option::or(google_client_secret.web, google_client_secret.installed)
            .ok_or_else(|| eyre!("no secret provided"))?,
        yup_oauth2::InstalledFlowReturnMethod::HTTPPortRedirect(8081),
    )
    .force_account_selection(true)
    .persist_tokens_to_disk("./tokens.json")
    .build()
    .await
    .unwrap();

    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .unwrap();
    // TODO: make sure using gzip (or other compression)
    let hyper_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build(
            hyper_rustls::HttpsConnectorBuilder::new()
                .with_native_roots()
                .unwrap()
                .https_only()
                .enable_http1()
                .enable_http2()
                .build(),
        );

    let youtube = YouTube::new(hyper_client, auth);

    let (new_video_sender, new_video_reciever) = tokio::sync::mpsc::channel(32);

    // TODO: some way to verify that the subscriptions are actually subscribed, maybe once a day?
    // https://pubsubhubbub.appspot.com/subscription-details?hub.callback=https%3A%2F%2Flenovo-fedora.taila5e2a.ts.net%2Fpubsub&hub.topic=https%3A%2F%2Fwww.youtube.com%2Fxml%2Ffeeds%2Fvideos.xml%3Fchannel_id%3DUCHtv-7yDeac7OSfPJA_a6aA&hub.secret=

    // Both web server and playlist modifier must update this....
    let subscriptions = Arc::new(Mutex::new(
        HashMap::<String, YoutubeChannelSubscription>::new(),
    ));

    let (shutdown, _) = tokio::sync::broadcast::channel(1);

    let mut pubsub_task = tokio::spawn(youtube_pubsub_reciever(
        shutdown.subscribe(),
        new_video_sender,
        subscriptions.clone(),
    ));
    let mut playlist_task = tokio::spawn(youtube_playlist_modifier(
        shutdown.subscribe(),
        client.clone(),
        youtube.clone(),
        subscriptions.clone(),
        Arc::from(playlist_id),
        new_video_reciever,
    ));
    let mut subscription_task = tokio::spawn(youtube_subscription_manager(
        shutdown.subscribe(),
        hostname,
        client,
        youtube,
        subscriptions,
    ));

    tokio::select! {
        result = &mut pubsub_task => tracing::error!(?result, "pubsub task exited"),
        result = &mut playlist_task => tracing::error!(?result, "playlist task exited"),
        result = &mut subscription_task => tracing::error!(?result, "subscription task exited"),
    }

    let _ = shutdown.send(());

    tokio::join!(pubsub_task, playlist_task, subscription_task).0??;

    Ok(())
}
