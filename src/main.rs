use std::{fs::File, io::BufReader, time::Duration};

use color_eyre::eyre::{Context, eyre};
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemContentDetails, PlaylistItemSnippet, ResourceId, Scope},
    yup_oauth2::{self, ConsoleApplicationSecret},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use reqwest::{StatusCode, header};
use tokio::{join, sync::mpsc::Receiver, try_join};
use tracing::info;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    feed::Feed,
    pubsub::{youtube_pubsub_reciever, youtube_pubsub_subscription_manager},
};

mod feed;
mod pubsub;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
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

    // TODO: make sure using gzip (or other compression)
    let client = reqwest::ClientBuilder::new()
        .https_only(true)
        .connector_layer(tower::limit::concurrency::ConcurrencyLimitLayer::new(10))
        .build()
        .wrap_err("Unable to setup reqwest client")?;

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

    let hub = YouTube::new(hyper_client, auth);

    let (new_video_sender, new_video_reciever) = tokio::sync::mpsc::channel(32);

    // TODO: should these be actors/tasks? the reciever is basically already one
    try_join!(
        youtube_pubsub_reciever(new_video_sender),
        // youtube_playlist_modifier(&client, hub.clone(), new_video_reciever),
        youtube_pubsub_subscription_manager(&client, hub)
    )
    .map(|_| ())
}

async fn youtube_playlist_modifier(
    http_client: &reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    reciever: Receiver<Feed>,
) -> color_eyre::Result<()> {
    let playlist_id = std::env::var("YOUTUBE_PLAYLIST_ID")
        .wrap_err("Unable to read YOUTUBE_PLAYLIST_ID env var")?;

    let (response, playlist_item) = youtube
        .playlist_items()
        .insert(PlaylistItem {
            content_details: Some(PlaylistItemContentDetails {
                note: Some("where does this show up".into()), // Does not persist
                ..Default::default()
            }),
            snippet: Some(PlaylistItemSnippet {
                playlist_id: Some(playlist_id.clone()),
                resource_id: Some(ResourceId {
                    kind: Some("youtube#video".into()),
                    video_id: Some("dQw4w9WgXcQ".into()),
                    ..Default::default()
                }),
                // position: todo!(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .doit()
        .await
        .wrap_err("failed to insert playlist item")?;

    dbg!(playlist_item);

    let (response, playlist_items) = youtube
        .playlist_items()
        .list(&vec!["snippet".into(), "contentDetails".into()])
        .max_results(10)
        .playlist_id(&playlist_id)
        .doit()
        .await
        .wrap_err("failed to fetch playlist information")?;

    dbg!(playlist_items);

    Ok(())
}
