use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    net::SocketAddr,
    str::FromStr as _,
};

use axum::{
    extract::{Query, State, rejection::QueryRejection},
    routing::method_routing,
};
use axum_extra::{TypedHeader, headers::ContentType};
use color_eyre::eyre::{Context as _, eyre};
use google_youtube3::{
    YouTube,
    api::{Scope, Subscription, SubscriptionListResponse},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use jiff::civil::DateTime;
use mime::Mime;
use quick_xml::DeError;
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{Receiver, Sender};
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{error, info, trace, warn};

use crate::feed::Feed;

#[derive(Debug, Deserialize)]
#[serde(tag = "hub.mode")]
pub enum HubChallenge {
    #[serde(rename = "subscribe")]
    Subscribe(HubSubscribeChallenge),
    #[serde(rename = "unsubscribe")]
    Unsubscribe(HubUnsubscribeChallenge),
}

// TODO: unsubscribe on shutdown???
#[derive(Debug, Deserialize)]
pub struct HubSubscribeChallenge {
    #[serde(rename = "hub.topic")]
    topic: String,
    #[serde(rename = "hub.challenge")]
    challenge: String,
    #[serde(rename = "hub.lease_seconds")]
    lease_seconds: String, // I think integers are special cased when at the root
}

#[derive(Debug, Deserialize)]
pub struct HubUnsubscribeChallenge {
    #[serde(rename = "hub.topic")]
    topic: String,
    #[serde(rename = "hub.challenge")]
    challenge: String,
}

pub async fn youtube_pubsub_reciever(new_video_channel: Sender<Feed>) -> color_eyre::Result<()> {
    async fn pubsub_subscription(
        query: Result<Query<HubChallenge>, QueryRejection>,
    ) -> Result<String, StatusCode> {
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
    }

    async fn pubsub_new_upload(
        // connect: ConnectInfo<SocketAddr>,
        // TypedHeader(user_agent): TypedHeader<UserAgent>,
        TypedHeader(content_type): TypedHeader<ContentType>,
        new_video_channel: State<Sender<Feed>>,
        body: String,
    ) -> StatusCode {
        if Mime::from(dbg!(content_type)) != Mime::from_str("application/atom+xml").unwrap() {
            return StatusCode::UNSUPPORTED_MEDIA_TYPE;
        }

        // TODO: verify remote IP, user agent and others??
        // tokio::net::lookup_host("pubsubhubbub.appspot.com").await

        let new_upload = match quick_xml::de::from_str::<Feed>(&body) {
            Ok(upload) => upload,
            Err(DeError::Custom(error)) => {
                warn!(?error, "unable to process valid xml feed item");
                return StatusCode::UNPROCESSABLE_ENTITY;
            }
            Err(error) => {
                warn!(%error, "unable to parse incoming feed item");
                return StatusCode::BAD_REQUEST;
            }
        };

        trace!(?new_upload, "Upload recieved");

        match new_video_channel.try_send(new_upload) {
            Ok(()) => StatusCode::ACCEPTED,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!("upload dropped due to queue being fill");
                StatusCode::TOO_MANY_REQUESTS
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                error!("upload dropped due to queue being closed");
                StatusCode::SERVICE_UNAVAILABLE
            }
        }
    }

    axum::serve(
        tokio::net::TcpListener::bind("0.0.0.0:8080")
            .await
            .wrap_err("unable to bind to port 8080")?,
        axum::Router::new()
            .route("/pubsub", {
                method_routing::get(pubsub_subscription)
                    .post(pubsub_new_upload)
                    .with_state(new_video_channel)
            })
            .fallback(method_routing::any(|| async {
                axum::http::StatusCode::PAYMENT_REQUIRED
            }))
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .wrap_err("failed to run axum server")
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Serialize)]
pub struct HubRequest {
    #[serde(rename = "hub.topic")]
    topic: String,
    #[serde(rename = "hub.callback")]
    callback: &'static str,
    #[serde(rename = "hub.mode")]
    mode: Mode,
    #[serde(rename = "hub.verify")]
    verify: Verify,
}

#[derive(Debug, Serialize)]
pub enum Verify {
    #[serde(rename = "async")]
    Asynchronous,
    #[serde(rename = "sync")]
    Synchronous,
}

pub async fn youtube_pubsub_subscription_manager(
    client: &reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
) -> color_eyre::Result<()> {
    let mut targeted_subscriptions = HashSet::<String>::new();
    let mut successful_subscriptions = HashMap::<String, DateTime>::new();

    let response = client
        .get("https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=5")
        .bearer_auth(
            youtube.auth.get_token(&[Scope::Readonly.as_ref()])
                .await.map_err(|e| eyre!("{e}")).wrap_err("unable to get authentication token")?.unwrap(), // TODO: FIXME: remove unwrap
        )
        .header(header::IF_NONE_MATCH, "U5Wg4I_OtB-xi1Diwh0Z6b3bGV0")
        .send()
        .await
        .unwrap();

    dbg!(&response);

    if response.status() == StatusCode::NOT_MODIFIED {
        info!("not changed"); // TODO: what
    } else {
        let json = response.json::<SubscriptionListResponse>().await.unwrap();

        dbg!(&json);

        info!("etag: {:#?}", json.etag);

        targeted_subscriptions.extend(json.items.unwrap().into_iter().map(|subscription| {
            let resource = subscription.snippet.unwrap().resource_id.unwrap();

            assert_eq!(resource.kind.as_deref(), Some("youtube#channel"));

            resource.channel_id.unwrap()
        }));

        info!("subscriptions: {targeted_subscriptions:?}")
    }

    // // TODO: use ETag to detect changes here
    // let (response, subscriptions) = youtube
    //     .subscriptions()
    //     .list(&vec!["snippet".into(), "contentDetails".into()])
    //     .mine(true)
    //     .max_results(5 /*50*/)
    //     .doit()
    //     .await
    //     .wrap_err("failed to fetch subscription information")?;

    // dbg!(subscriptions);

    let subscription_delta = [(Mode::Subscribe, "UCHtv-7yDeac7OSfPJA_a6aA")].into_iter();

    for (mode, channel_id) in subscription_delta {
        let request = client
            .post("https://pubsubhubbub.appspot.com/subscribe")
            .form(&HubRequest {
                mode,
                verify: Verify::Synchronous,
                callback: "https://lenovo-fedora.taila5e2a.ts.net/pubsub",
                topic: format!(
                    "https://www.youtube.com/xml/feeds/videos.xml?channel_id={channel_id}"
                ),
            })
            .build()
            .expect("request should be well formed");

        let response = match client.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                // TODO: implement retries? put back on the queue?
                // TODO: keep track of subscribed channels?? how do we know whats new?
                warn!(%error, "failed to subscribe to a youtube channel");
                continue;
            }
        };

        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            warn!(status_code, "server returned error");
            continue;
        }

        info!(channel_id, "subscribed to channel")
    }

    Ok(())
}
