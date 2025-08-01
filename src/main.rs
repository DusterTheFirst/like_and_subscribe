use std::{collections::HashMap, fs::File, io::BufReader, sync::{Arc, Mutex}, time::Duration};

use color_eyre::eyre::{Context, eyre};
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemContentDetails, PlaylistItemSnippet, ResourceId},
    yup_oauth2::{self, ConsoleApplicationSecret},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use tokio::{sync::mpsc::Receiver, try_join};
use tower::ServiceBuilder;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    feed::Feed,
    pubsub::{youtube_pubsub_reciever, youtube_pubsub_subscription_manager, YoutubeChannelSubscription},
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
        .connector_layer(
            ServiceBuilder::new()
                .buffer(1024)
                .rate_limit(5, Duration::from_secs(10))
                .concurrency_limit(10),
        )
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

    let youtube = YouTube::new(hyper_client, auth);

    let (new_video_sender, new_video_reciever) = tokio::sync::mpsc::channel(32);


    // TODO: some way to verify that the subscriptions are actually subscribed, maybe once a day?
    // https://pubsubhubbub.appspot.com/subscription-details?hub.callback=https%3A%2F%2Flenovo-fedora.taila5e2a.ts.net%2Fpubsub&hub.topic=https%3A%2F%2Fwww.youtube.com%2Fxml%2Ffeeds%2Fvideos.xml%3Fchannel_id%3DUCHtv-7yDeac7OSfPJA_a6aA&hub.secret=

    // Both web server and playlist modifier must update this....
    let subscriptions = Arc::new(Mutex::new(HashMap::<String, YoutubeChannelSubscription>::new()));

    // TODO: should these be actors/tasks? the reciever is basically already one
    try_join!(
        youtube_pubsub_reciever(new_video_sender, subscriptions.clone()),
        youtube_playlist_modifier(&client, youtube.clone(), playlist_id, new_video_reciever),
        youtube_pubsub_subscription_manager(&client, youtube, &subscriptions)
    )
    .map(|_| ())
}

async fn youtube_playlist_modifier(
    http_client: &reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    playlist_id: String,
    mut reciever: Receiver<Feed>,
) -> color_eyre::Result<()> {
    while let Some(feed) = reciever.recv().await {
        // TODO: verify that it is not a short, and it is a new upload, not a modified old one
        dbg!(feed);
    }

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
