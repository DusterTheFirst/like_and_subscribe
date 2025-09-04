use std::{
    collections::HashMap,
    net::SocketAddr,
    str::FromStr as _,
    sync::{Arc, Mutex},
};

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    routing::method_routing,
};
use axum_extra::{TypedHeader, headers::ContentType};
use color_eyre::eyre::Context as _;
use jiff::{Timestamp, Zoned, tz::TimeZone};
use mime::Mime;
use quick_xml::DeError;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc::Sender;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{debug_span, error, trace, warn};

use super::subscription::YoutubeChannelSubscription;
use crate::feed::Feed;

#[derive(Debug, Deserialize)]
#[serde(tag = "hub.mode")]
pub enum HubChallenge {
    #[serde(rename = "subscribe")]
    Subscribe(HubSubscribeChallenge),
    #[serde(rename = "unsubscribe")]
    Unsubscribe(HubUnsubscribeChallenge),
}

#[derive(Debug, Deserialize)]
pub struct HubSubscribeChallenge {
    #[serde(rename = "hub.topic")]
    pub(crate) topic: String,
    #[serde(rename = "hub.challenge")]
    pub(crate) challenge: String,
    #[serde(rename = "hub.lease_seconds")]
    pub(crate) lease_seconds: String, // I think integers are special cased when at the root
}

#[derive(Debug, Deserialize)]
pub struct HubUnsubscribeChallenge {
    #[serde(rename = "hub.topic")]
    pub(crate) topic: String,
    #[serde(rename = "hub.challenge")]
    pub(crate) challenge: String,
}

pub async fn pubsub_subscription(
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
                jiff::Span::new().seconds(
                    query
                        .lease_seconds
                        .parse::<i64>()
                        .expect("lease seconds should always be a number"),
                ),
            );

            trace!(topic = query.topic, %expiration, "validating subscription");
            match subscriptions
                .lock()
                .expect("mutex should not be poisoned")
                .get_mut(id)
            {
                Some(channel) => {
                    trace!(topic = query.topic, %expiration, "subscription expected");
                    channel.subscription_expiration = Some(expiration);

                    Ok(query.challenge)
                }
                None => {
                    warn!(topic = query.topic, %expiration, "subscription unexpected");
                    Err(StatusCode::NOT_FOUND)
                }
            }
        }
        Err(error) => {
            warn!(%error, "recieved bad request to pubsub route");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

pub async fn pubsub_new_upload(
    // connect: ConnectInfo<SocketAddr>,
    // TypedHeader(user_agent): TypedHeader<UserAgent>,
    TypedHeader(content_type): TypedHeader<ContentType>,
    new_video_channel: State<Sender<(tracing::Span, Feed)>>,
    body: String,
) -> StatusCode {
    if Mime::from(content_type)
        != Mime::from_str("application/atom+xml").expect("mime should be valid")
    {
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

    let span = debug_span!(
        "new_feed_item",
        updated = %feed.entry.updated,
        published = %feed.entry.published,
        video_id = feed.entry.video_id,
        channel_id = feed.entry.channel_id,
        title = feed.entry.title,
        channel_name = tracing::field::Empty,
        video_age_minutes = tracing::field::Empty,
        inserted = false,
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
