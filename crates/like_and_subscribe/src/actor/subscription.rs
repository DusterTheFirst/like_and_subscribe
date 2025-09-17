use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use axum::http::{HeaderMap, HeaderValue};
use entity::known_channels;
use entity_types::subscription_queue::SubscriptionAction;
use google_youtube3::api::SubscriptionListResponse;
use oauth2::AccessToken;
use reqwest::{StatusCode, header};
use sea_orm::{DatabaseConnection, DbErr};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{
    database::{ActiveSubscriptions, KnownChannels, SubscriptionQueue},
    oauth::TokenManager,
};

pub async fn subscription_manager(
    shutdown: CancellationToken,
    database: DatabaseConnection,
    notify: Arc<Notify>,
    client: reqwest::Client,
    token_manager: TokenManager,
) -> Result<(), DbErr> {
    // One hour
    let mut update_interval = tokio::time::interval(Duration::from_secs(60 * 60));
    update_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_etag: Option<String> = None;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = update_interval.tick() => {},
        }

        let previous_channel_ids = ActiveSubscriptions::get_all_channel_ids(&database)
            .await
            .inspect_err(|error| tracing::error!(%error, "failed to get all channel ids"))?;

        let token = tokio::select! {
            _ = shutdown.cancelled() => break,
            token_result = token_manager.wait_for_token() => token_result.inspect_err(|error| tracing::error!(%error, "failed to get current token"))?,
        };

        let current_channels = match get_all_subscriptions(&client, &mut last_etag, token).await {
            Some(channel_ids) => channel_ids,
            None => break, // TODO: this is both on error and on no update
        };
        let current_channel_ids = HashSet::from_iter(current_channels.keys().cloned());

        let added_channels = current_channel_ids.difference(&previous_channel_ids);
        let removed_channels = previous_channel_ids.difference(&current_channel_ids);

        let added_actions =
            added_channels.map(|channel_id| (channel_id.clone(), SubscriptionAction::Subscribe));
        let removed_actions = removed_channels
            .map(|channel_id| (channel_id.clone(), SubscriptionAction::Unsubscribe));

        SubscriptionQueue::add_actions(&database, &notify, added_actions.chain(removed_actions))
            .await
            .inspect_err(
                |error| tracing::error!(%error, "failed to add actions to subscription queue"),
            )?;

        let updated_channels =
            current_channels
                .into_iter()
                .map(|(channel_id, metadata)| known_channels::Model {
                    channel_id: channel_id.clone(),
                    channel_name: metadata.name,
                    channel_profile_picture: metadata.profile_picture,
                });

        KnownChannels::add_channels(&database, updated_channels)
            .await
            .inspect_err(
                |error| tracing::error!(%error, "failed to add new channels to known channels list"),
            )?;
    }

    tracing::info!("shutting down");

    Ok(())
}

struct ChannelMetadata {
    name: String,
    profile_picture: String,
}

async fn get_all_subscriptions(
    client: &reqwest::Client,
    last_etag: &mut Option<String>,
    token: AccessToken,
) -> Option<HashMap<String, ChannelMetadata>> {
    let mut page_token = None;
    let url = "https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=50";

    let mut channel_ids = HashMap::new();

    // Pagination handling
    loop {
        let url = if let Some(page_token) = &page_token {
            Cow::Owned(format!("{url}&pageToken={page_token}"))
        } else {
            Cow::Borrowed(url)
        };

        let headers = if let Some(etag) = last_etag {
            HeaderMap::from_iter([(header::IF_NONE_MATCH, HeaderValue::from_str(etag).unwrap())])
        } else {
            HeaderMap::new()
        };

        let response = client
            .get(url.as_ref())
            .bearer_auth(token.secret())
            .headers(headers)
            .send()
            .await
            .unwrap();

        let status = response.status();

        if status == StatusCode::NOT_MODIFIED {
            // TODO: in database?
            tracing::info!("not changed");
            break None;
        }

        if !status.is_success() {
            // TODO: in database?
            tracing::warn!(status=%status, status_message=status.canonical_reason(), "failed to paginate all subscriptions");
            break None;
        }

        let json = response.json::<SubscriptionListResponse>().await.unwrap();

        if page_token.is_none() {
            // Update first etag
            *last_etag = json.etag;
        }

        // let total_results = json.page_info.unwrap().total_results.unwrap();
        let items = json.items.unwrap();

        for subscription in items {
            let snippet = subscription.snippet.unwrap();
            let resource = snippet.resource_id.unwrap();

            debug_assert_eq!(resource.kind.as_deref(), Some("youtube#channel"));

            let channel_id = resource.channel_id.unwrap();
            let channel_name = snippet.title.unwrap();
            let channel_thumbnail = {
                let thumbnail = snippet.thumbnails.unwrap();

                thumbnail
                    .default
                    .or(thumbnail.standard)
                    .or(thumbnail.medium)
                    .or(thumbnail.high)
                    .or(thumbnail.maxres)
                    .expect("one of the thumbnails should exist") // TODO: throw error? put in database??/ log better?
            };

            channel_ids.insert(
                channel_id,
                ChannelMetadata {
                    name: channel_name,
                    profile_picture: channel_thumbnail.url.unwrap(),
                },
            );
        }

        page_token = json.next_page_token;

        if page_token.is_none() {
            break Some(channel_ids);
        }
    }
}
