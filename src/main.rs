use std::{fs::File, io::BufReader, net::SocketAddr};

use axum::{
    extract::{ConnectInfo, Query, Request, rejection::QueryRejection},
    routing::method_routing,
};
use axum_extra::{
    TypedHeader,
    headers::{self, ContentType, UserAgent},
};
use color_eyre::eyre::{Context, eyre};
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemContentDetails, PlaylistItemSnippet, ResourceId, Scope},
    yup_oauth2::{self, ConsoleApplicationSecret},
};
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{info, trace, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::feed::Feed;

mod feed;

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

    let client = reqwest::ClientBuilder::new()
        .https_only(true)
        .build()
        .wrap_err("Unable to setup reqwest client")?;

    // youtube_playlist(&client, google_client_secret).await?;
    youtube_pubsub(&client).await?;

    Ok(())
}

async fn youtube_pubsub(client: &reqwest::Client) -> color_eyre::Result<()> {
    async fn pubsub_post(
        connect: ConnectInfo<SocketAddr>,
        TypedHeader(user_agent): TypedHeader<UserAgent>,
        TypedHeader(content_type): TypedHeader<ContentType>,
        body: String,
    ) {
        trace!("Post");

        // TODO: verify remote IP, user agent and others??
        // tokio::net::lookup_host("pubsubhubbub.appspot.com").await

        dbg!(quick_xml::de::from_str::<Feed>(&body));
    }

    let web_server = tokio::task::spawn(async {
        axum::serve(
            tokio::net::TcpListener::bind("0.0.0.0:8080")
                .await
                .wrap_err("unable to bind to port 8080")?,
            axum::Router::new()
                .route("/pubsub", {
                    method_routing::get(
                        |query: Result<Query<HubChallenge>, QueryRejection>| async move {
                            match query {
                                Ok(Query(HubChallenge::Unsubscribe(query))) => {
                                    trace!(?query, "validating unsubscription");
                                    Ok(query.challenge)
                                }
                                Ok(Query(HubChallenge::Subscribe(query))) => {
                                    trace!(?query, "validating subscription");
                                    Ok(query.challenge)
                                }
                                Err(error) => {
                                    warn!(%error, "recieved bad request to pubsub route");
                                    Err(StatusCode::BAD_REQUEST)
                                }
                            }
                        },
                    )
                    .post(pubsub_post)
                })
                .fallback(method_routing::any(|r: Request| async {
                    axum::http::StatusCode::PAYMENT_REQUIRED
                }))
                .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .wrap_err("failed to run axum server")
    });

    #[derive(Debug, Deserialize)]
    #[serde(tag = "hub.mode")]
    enum HubChallenge {
        #[serde(rename = "subscribe")]
        Subscribe(HubSubscribeChallenge),
        #[serde(rename = "unsubscribe")]
        Unsubscribe(HubUnsubscribeChallenge),
    }

    // TODO: unsubscribe on shutdown???
    #[derive(Debug, Deserialize)]
    struct HubSubscribeChallenge {
        #[serde(rename = "hub.topic")]
        topic: String,
        #[serde(rename = "hub.challenge")]
        challenge: String,
        #[serde(rename = "hub.lease_seconds")]
        lease_seconds: String, // I think integers are special cased when at the root
    }

    #[derive(Debug, Deserialize)]
    struct HubUnsubscribeChallenge {
        #[serde(rename = "hub.topic")]
        topic: String,
        #[serde(rename = "hub.challenge")]
        challenge: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(rename_all = "lowercase")]
    enum Mode {
        Subscribe,
        Unsubscribe,
    }

    #[derive(Debug, Serialize)]
    struct HubRequest {
        #[serde(rename = "hub.topic")]
        topic: &'static str,
        #[serde(rename = "hub.callback")]
        callback: &'static str,
        #[serde(rename = "hub.mode")]
        mode: Mode,
        #[serde(rename = "hub.verify")]
        verify: Verify,
    }

    #[derive(Debug, Serialize)]
    enum Verify {
        #[serde(rename = "async")]
        Asynchronous,
        #[serde(rename = "sync")]
        Synchronous,
    }

    let response = client.execute(
            client
                .post("https://pubsubhubbub.appspot.com/subscribe")
                .form(
                   &HubRequest {
                        mode: Mode::Subscribe,
                        verify: Verify::Synchronous,
                        callback: "https://lenovo-fedora.taila5e2a.ts.net/pubsub",
                        topic: "https://www.youtube.com/xml/feeds/videos.xml?channel_id=UCHtv-7yDeac7OSfPJA_a6aA"
                    }
                )
                .build()
                .unwrap(),
        ).await?;

    dbg!(&response);

    web_server.await??;

    Ok(())
}

async fn youtube_playlist(
    http_client: &reqwest::Client,
    google_client_secret: ConsoleApplicationSecret,
) -> color_eyre::Result<()> {
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
    // TODO: make sure using gzip (or other compression)
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

    let response = http_client
        .get("https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=5")
        .bearer_auth(
            auth.token(&[Scope::Readonly.as_ref()])
                .await
                .unwrap()
                .token()
                .unwrap(),
        )
        .header(header::IF_NONE_MATCH, "U5Wg4I_OtB-xi1Diwh0Z6b3bGV0")
        .send()
        .await
        .unwrap();

    dbg!(&response);

    if response.status() == StatusCode::NOT_MODIFIED {
        info!("not changed");
    }

    let hub = YouTube::new(client, auth);

    // TODO: use ETag to detect changes here
    let (response, subscriptions) = hub
        .subscriptions()
        .list(&vec!["snippet".into(), "contentDetails".into()])
        .mine(true)
        .max_results(5 /*50*/)
        .doit()
        .await
        .wrap_err("failed to fetch subscription information")?;

    dbg!(subscriptions);

    return Ok(());

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
