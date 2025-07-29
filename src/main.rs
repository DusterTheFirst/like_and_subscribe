use std::{fs::File, io::BufReader};

use color_eyre::eyre::{Context, eyre};
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemContentDetails, PlaylistItemSnippet, ResourceId},
    yup_oauth2::{self, ConsoleApplicationSecret},
};
use tokio::{join, try_join};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let google_client_secret: ConsoleApplicationSecret = serde_json::from_reader(BufReader::new(
        File::open(
            std::env::var("GOOGLE_CLIENT_SECRET_FILE")
                .wrap_err("Unable to read GOOGLE_CLIENT_SECRET_FILE env var")?,
        )
        .wrap_err("unable to open google client secret file")?,
    ))
    .wrap_err("unable to parse google client secret file")?;

    let client = reqwest::ClientBuilder::new()
        .https_only(true)
        .default_headers([].into_iter().collect())
        .build()
        .wrap_err("Unable to setup reqwest client")?;

    println!("Hello, world!");

    try_join!(server(), youtube(google_client_secret)).map(|_| ())
}

async fn server() -> color_eyre::Result<()> {
    axum::serve(
        tokio::net::TcpListener::bind("0.0.0.0:8080")
            .await
            .wrap_err("unable to bind to port 8080")?,
        axum::Router::new(),
    )
    .await
    .wrap_err("failed to run axum server")
}

async fn youtube(google_client_secret: ConsoleApplicationSecret) -> color_eyre::Result<()> {
    // Instantiate the authenticator. It will choose a suitable authentication flow for you,
    // unless you replace  `None` with the desired Flow.
    // Provide your own `AuthenticatorDelegate` to adjust the way it operates and get feedback about
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
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(
            hyper_rustls::HttpsConnectorBuilder::new()
                .with_native_roots()
                .unwrap()
                .https_only()
                .enable_http1()
                .enable_http2()
                .build(),
        );
    let mut hub = YouTube::new(client, auth);

    let playlist_id = std::env::var("YOUTUBE_PLAYLIST_ID")
        .wrap_err("Unable to read YOUTUBE_PLAYLIST_ID env var")?;

    let (response, playlist_item) = hub
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

    let (response, playlist_items) = hub
        .playlist_items()
        .list(&vec!["snippet".into(), "contentDetails".into()])
        .max_results(10)
        .playlist_id(&playlist_id)
        .doit()
        .await
        .wrap_err("failed to fetch playlist information")?;

    dbg!(playlist_items);

    // return Ok(());

    let (response, playlists) = hub
        .playlists()
        .list(&vec!["snippet".into(), "contentDetails".into()])
        .max_results(20)
        .mine(true)
        .doit()
        .await
        .wrap_err("failed to fetch playlists")?;

    dbg!(&playlists);

    for playlist in playlists.items.iter().flatten() {
        let snippet = playlist.snippet.as_ref().unwrap();
        let content_details = playlist.content_details.as_ref().unwrap();
        println!(
            "{} {:>30} ({:>4}) @ {} | {}",
            playlist.id.as_ref().unwrap(),
            snippet.title.as_ref().unwrap(),
            content_details.item_count.unwrap_or_default(),
            snippet.published_at.as_ref().unwrap(),
            snippet.description.as_ref().unwrap()
        )
    }

    Ok(())
}
