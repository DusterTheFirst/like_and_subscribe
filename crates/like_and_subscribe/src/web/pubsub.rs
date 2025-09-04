use std::{str::FromStr as _, sync::Arc};

use axum::extract::{Query, State, rejection::QueryRejection};
use axum_extra::{TypedHeader, headers::ContentType};
use jiff::Timestamp;
use jiff::Zoned;
use jiff_sea_orm_compat::JiffTimestampMilliseconds;
use mime::Mime;
use quick_xml::DeError;
use reqwest::StatusCode;
use sea_orm::ActiveValue;
use sea_orm::DatabaseConnection;
use sea_orm::EntityTrait;
use sea_orm::IntoActiveModel;
use serde::Deserialize;
use tokio::sync::Notify;
use tracing::warn;

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

fn channel_id_from_topic(topic: &str) -> &str {
    topic
        // FIXME: poor man's url parser
        .trim_start_matches("https://www.youtube.com/xml/feeds/videos.xml?channel_id=")
}

pub async fn pubsub_subscription_validation(
    query: Result<Query<HubChallenge>, QueryRejection>,
    State(database): State<DatabaseConnection>,
) -> Result<String, StatusCode> {
    match query {
        Ok(Query(HubChallenge::Unsubscribe(query))) => {
            let database_result = entity::active_subscriptions::Entity::delete_by_id(
                channel_id_from_topic(&query.topic).to_owned(),
            )
            .exec(&database)
            .await;

            match database_result {
                Ok(_) => Ok(query.challenge),
                Err(error) => {
                    tracing::error!(%error, "failed to remove active subscription");
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Ok(Query(HubChallenge::Subscribe(query))) => {
            let channel_id = channel_id_from_topic(&query.topic);

            let expiration = Zoned::now()
                .saturating_add(
                    jiff::Span::new().seconds(
                        query
                            .lease_seconds
                            .parse::<i64>()
                            .expect("lease seconds should always be a number"),
                    ),
                )
                .timestamp();

            let database_result = entity::active_subscriptions::Entity::insert(
                entity::active_subscriptions::Model {
                    channel_id: channel_id.to_owned(),
                    expiration: JiffTimestampMilliseconds(expiration),
                }
                .into_active_model(),
            )
            .exec(&database)
            .await;

            match database_result {
                Ok(_) => Ok(query.challenge),
                Err(error) => {
                    tracing::error!(%error, "failed to add active subscription");
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
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
    State((database, notification)): State<(DatabaseConnection, Arc<Notify>)>,
    body: String,
) -> StatusCode {
    if Mime::from(content_type)
        != Mime::from_str("application/atom+xml").expect("mime should be valid")
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE;
    }

    // TODO: verify remote IP, user agent and others??
    // tokio::net::lookup_host("pubsubhubbub.appspot.com").await

    // TODO: store bad XML feed items in database instead of logging or something for debugging (due to "missing field `@xmlns:yt`")
    let feed = match quick_xml::de::from_str::<Feed>(&body) {
        Ok(feed) => feed,
        Err(DeError::Custom(error)) => {
            warn!(%error, %body, "unable to process valid xml feed item");
            return StatusCode::UNPROCESSABLE_ENTITY;
        }
        Err(error) => {
            warn!(%error, %body, "unable to parse incoming feed item");
            return StatusCode::BAD_REQUEST;
        }
    };

    let database_result = entity::video_queue::Entity::insert(entity::video_queue::ActiveModel {
        id: ActiveValue::NotSet,
        channel_id: ActiveValue::Set(feed.entry.channel_id),
        video_id: ActiveValue::Set(feed.entry.video_id),

        title: ActiveValue::Set(feed.entry.title),

        published_at: ActiveValue::Set(JiffTimestampMilliseconds(feed.entry.published)),
        updated_at: ActiveValue::Set(JiffTimestampMilliseconds(feed.entry.updated)),

        timestamp: ActiveValue::Set(JiffTimestampMilliseconds(Timestamp::now())),
    })
    .exec(&database)
    .await;

    if let Err(error) = database_result {
        tracing::error!(%error, "failed to insert video into queue");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    notification.notify_waiters();

    StatusCode::ACCEPTED
}
