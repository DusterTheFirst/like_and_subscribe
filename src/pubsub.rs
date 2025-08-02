use std::{
    collections::HashMap,
    net::SocketAddr,
    str::FromStr as _,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    http::HeaderValue,
    routing::method_routing,
};
use axum_extra::{TypedHeader, headers::ContentType};
use color_eyre::eyre::{Context as _, eyre};
use google_youtube3::{
    YouTube,
    api::{Scope, SubscriptionListResponse},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use jiff::{Span, Timestamp, Zoned, tz::TimeZone};
use mime::Mime;
use quick_xml::DeError;
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc::Sender, task::JoinSet};
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{Instrument, debug, error, info, trace, trace_span, warn};

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

pub async fn youtube_pubsub_reciever(
    new_video_channel: Sender<(tracing::Span, Feed)>,
    subscriptions: Arc<Mutex<HashMap<String, YoutubeChannelSubscription>>>,
) -> color_eyre::Result<()> {
    async fn pubsub_subscription(
        query: Result<Query<HubChallenge>, QueryRejection>,
        State(subscriptions): State<Arc<Mutex<HashMap<String, YoutubeChannelSubscription>>>>,
    ) -> Result<String, StatusCode> {
        match query {
            Ok(Query(HubChallenge::Unsubscribe(query))) => {
                trace!(topic = query.topic, "validating unsubscription");
                Ok(query.challenge)
            }
            Ok(Query(HubChallenge::Subscribe(query))) => {
                let id = query
                    .topic
                    // FIXME: poor man's url parser
                    .trim_start_matches("https://www.youtube.com/xml/feeds/videos.xml?channel_id=");

                let expiration = Zoned::now().saturating_add(
                    Span::new().seconds(
                        query
                            .lease_seconds
                            .parse::<i64>()
                            .expect("lease seconds should always be a number"),
                    ),
                );

                trace!(topic = query.topic, %expiration, "validating subscription");

                subscriptions
                    .lock()
                    .unwrap()
                    .entry(id.to_string())
                    .or_default()
                    .subscription_expiration = Some(expiration);

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
        new_video_channel: State<Sender<(tracing::Span, Feed)>>,
        body: String,
    ) -> StatusCode {
        if Mime::from(content_type) != Mime::from_str("application/atom+xml").unwrap() {
            return StatusCode::UNSUPPORTED_MEDIA_TYPE;
        }

        // TODO: verify remote IP, user agent and others??
        // tokio::net::lookup_host("pubsubhubbub.appspot.com").await

        let feed = match quick_xml::de::from_str::<Feed>(&body) {
            Ok(feed) => feed,
            Err(DeError::Custom(error)) => {
                warn!(?error, "unable to process valid xml feed item");
                return StatusCode::UNPROCESSABLE_ENTITY;
            }
            Err(error) => {
                warn!(%error, "unable to parse incoming feed item");
                return StatusCode::BAD_REQUEST;
            }
        };

        let span = trace_span!(
            "new_feed_item",
            updated = %feed.entry.updated,
            published = %feed.entry.published,
            video_id = feed.entry.video_id,
            channel_id = feed.entry.channel_id,
            title = feed.entry.title,
            video_age_minutes = tracing::field::Empty,
            inserted = tracing::field::Empty,
        );

        match new_video_channel.try_send((span, feed)) {
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
                    .with_state(subscriptions.clone())
                    .post(pubsub_new_upload)
                    .with_state(new_video_channel)
            })
            .route(
                "/debug",
                method_routing::get(
                    |State(subscriptions): State<
                        Arc<Mutex<HashMap<String, YoutubeChannelSubscription>>>,
                    >| async move {
                        let subscriptions = HashMap::clone(&subscriptions.lock().unwrap());
                        let (subscribed, soonest_expiration, latest_expiration) =
                            subscriptions.values().fold(
                                (
                                    0,
                                    Zoned::new(Timestamp::MAX, TimeZone::system()),
                                    Zoned::new(Timestamp::MIN, TimeZone::system()),
                                ),
                                |(subscribed, soonest_expiration, latest_expiration), s| {
                                    if let Some(expiration) = s.subscription_expiration.as_ref() {
                                        (
                                            subscribed + 1,
                                            Zoned::min(soonest_expiration, expiration.clone()),
                                            Zoned::max(latest_expiration, expiration.clone()),
                                        )
                                    } else {
                                        (subscribed, soonest_expiration, latest_expiration)
                                    }
                                },
                            );

                        Json(json!({
                            "stats": {
                                "subscribed": subscribed,
                                "total": subscriptions.len(),
                                "expiration": {
                                    "soonest": soonest_expiration,
                                    "latest": latest_expiration
                                }
                            },
                            "subscriptions":subscriptions
                        }))
                    },
                )
                .with_state(subscriptions),
            )
            .fallback(method_routing::any(|| async {
                axum::http::StatusCode::PAYMENT_REQUIRED
            }))
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .wrap_err("failed to run axum server")
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
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

#[derive(Debug, Serialize, Clone, Copy)]
pub enum Verify {
    #[serde(rename = "async")]
    Asynchronous,
    #[serde(rename = "sync")]
    Synchronous,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct YoutubeChannelSubscription {
    pub name: String,
    pub subscription_expiration: Option<Zoned>,
    pub stale: bool,
}

pub async fn youtube_pubsub_subscription_manager(
    client: &reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    subscriptions: &Mutex<HashMap<String, YoutubeChannelSubscription>>,
) -> color_eyre::Result<()> {
    let mut last_etag: Option<String> = None;

    let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60)); // One hour
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        let token = youtube
            .auth
            .get_token(&[Scope::Readonly.as_ref()])
            .await
            .map_err(|e| eyre!("{e}"))
            .wrap_err("unable to get authentication token")?
            .unwrap(); // TODO: FIXME: remove unwrap

        // Mark all existing subscriptions stale
        subscriptions
            .lock()
            .unwrap()
            .values_mut()
            .for_each(|s| s.stale = true);

        let mut page_token = None;
        let url = "https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=50";

        // Pagination handling
        loop {
            let url = if let Some(page_token) = &page_token {
                format!("{url}&pageToken={page_token}")
            } else {
                url.to_string()
            };

            let response = client
                .get(url)
                .bearer_auth(&token)
                .headers(
                    [last_etag
                        .as_ref()
                        .map(|etag| (header::IF_NONE_MATCH, HeaderValue::from_str(etag).unwrap()))]
                    .into_iter()
                    .flatten()
                    .collect(),
                )
                .send()
                .await
                .unwrap();

            if response.status() == StatusCode::NOT_MODIFIED {
                info!("not changed");

                // Mark all existing subscriptions as not stale
                subscriptions
                    .lock()
                    .unwrap()
                    .values_mut()
                    .for_each(|s| s.stale = false);

                break;
            }

            // TODO: check for error response too

            let json = response.json::<SubscriptionListResponse>().await.unwrap();

            info!(json.etag, json.next_page_token);

            if page_token.is_none() {
                last_etag = json.etag;
            }

            let items = json.items.unwrap();

            let mut subscriptions = subscriptions.lock().unwrap();
            for subscription in items {
                let snippet = subscription.snippet.unwrap();
                let resource = snippet.resource_id.unwrap();

                assert_eq!(resource.kind.as_deref(), Some("youtube#channel"));

                let channel_id = resource.channel_id.unwrap();
                let channel_name = snippet.title.unwrap();

                // Either add item or mark as fresh
                match subscriptions.entry(channel_id.clone()) {
                    std::collections::hash_map::Entry::Occupied(mut occupied_entry) => {
                        occupied_entry.get_mut().stale = false;
                    }
                    std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(YoutubeChannelSubscription {
                            name: channel_name,
                            subscription_expiration: None,
                            stale: false,
                        });
                    }
                }
            }

            page_token = json.next_page_token;

            if page_token.is_none() {
                break;
            }
        }

        // Prune stale entries
        {
            let mut action_queue = subscriptions
                .lock()
                .unwrap()
                .extract_if(|_, sub| sub.stale)
                .inspect(|(channel_id, sub)| {
                    debug!(?channel_id, name = sub.name, "removing stale subscription");
                })
                .map(|x| (Mode::Unsubscribe, x))
                .collect::<Vec<_>>();

            action_queue.extend(
                subscriptions
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, s)| match s.subscription_expiration.as_ref() {
                        Some(expiration) => {
                            let now = Zoned::now();

                            // re-subscribe if expring in a day
                            expiration.duration_since(&now)
                                <= Span::new().days(1).to_duration(&now).unwrap()
                        }
                        None => true,
                    })
                    .map(|(a, b)| (Mode::Subscribe, (a.clone(), b.clone()))),
            );

            let mut set = action_queue
                .into_iter().map(|(mode, (channel_id, YoutubeChannelSubscription { name, .. }))| {
                    let client = client.clone();

                    let span = trace_span!("subscription_update", channel_id, name, ?mode);

                    async move {
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
                                return;
                            }
                        };

                        if response.status() == StatusCode::TOO_MANY_REQUESTS {
                            // TODO: retries from too many requests
                            error!("too many requests");
                            todo!();
                        }

                        if !response.status().is_success() {
                            let status_code = response.status().as_u16();
                            warn!(status_code, "server returned error");
                            return;
                        }

                        trace!("end")
                    }
                    .instrument(span)
                })
                .collect::<JoinSet<_>>();

            while set.join_next().await.is_some() {}
        }

        let subscriptions = subscriptions.lock().unwrap();
        let total_count = subscriptions.len();
        let stale_count = subscriptions.values().filter(|s| s.stale).count();
        let subscribed_count = subscriptions
            .values()
            .filter(|s| s.subscription_expiration.is_some())
            .count();
        let soonest_expiration = subscriptions
            .values()
            .flat_map(|s| s.subscription_expiration.as_ref())
            .max()
            .map(|exp| exp.to_string());

        info!(
            total_count,
            stale_count, subscribed_count, soonest_expiration, "subscription update end"
        );
    }
}
