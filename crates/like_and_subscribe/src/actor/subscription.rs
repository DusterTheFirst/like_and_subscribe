use std::{
    borrow::Cow, collections::{HashMap, HashSet}, sync::Arc, time::Duration
};

use axum::http::{HeaderMap, HeaderValue};
use futures::{StreamExt, stream};
use google_youtube3::api::{ChannelListResponse, SubscriptionListResponse};
use oauth2::AccessToken;
use reqwest::{StatusCode, header};
use tokio_util::sync::CancellationToken;

use crate::{database::Database, oauth::TokenManager};

pub async fn subscription_manager(
    shutdown: CancellationToken,
    database: Database,
    client: reqwest::Client,
    token_manager: TokenManager,
    playlist_id: Arc<str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // One hour
    let mut update_interval = tokio::time::interval(Duration::from_secs(60 * 60));
    update_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_etag: Option<String> = None;

    let mut subscribed_channels: HashMap<String, ChannelMetadata> = HashMap::new();
    let mut subscribed_channels_playlist: HashMap<String, String> = HashMap::new();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = update_interval.tick() => {},
        }

        let token = tokio::select! {
            _ = shutdown.cancelled() => break,
            token_result = token_manager.wait_for_token() => token_result.inspect_err(|error| tracing::error!(%error, "failed to get current token"))?,
        };

        match get_all_subscriptions(&client, &mut last_etag, &token).await {
            Ok(Some(subscriptions)) => subscribed_channels = subscriptions,
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "failed to paginate all subscriptions");
                continue;
            }
        };

        let subscribed_channel_ids: HashSet<&str> =
            subscribed_channels.keys().map(String::as_str).collect();
        let subscribed_channels_playlist_ids: HashSet<&str> = subscribed_channels_playlist
            .keys()
            .map(String::as_str)
            .collect();

        let channels_requiring_playlist_info: Vec<String> = subscribed_channel_ids
            .difference(&subscribed_channels_playlist_ids)
            .copied()
            .map(String::from)
            .collect();

        for channel_ids in channels_requiring_playlist_info.chunks(50) {
            let url = format!(
                "https://www.googleapis.com/youtube/v3/channels?part=contentDetails,id&maxResults=50&id={}",
                channel_ids.join(",")
            );

            let response = client
                .get(url)
                .bearer_auth(token.secret())
                .send()
                .await?
                .error_for_status()?;

            let json = response.json::<ChannelListResponse>().await?;

            for item in json.items.unwrap() {
                subscribed_channels_playlist.insert(
                    item.id.unwrap(),
                    item.content_details
                        .unwrap()
                        .related_playlists
                        .unwrap()
                        .uploads
                        .unwrap(),
                );
            }
        }

        dbg!(subscribed_channels_playlist);

        panic!();

        // stream::iter(&subscribed_channels)
        //     .for_each_concurrent(5, async |(channel_id, channel_info)| {
        //         let playlist_url = subscribed_channels_playlist.get(channel_id).unwrap(); // must be set previously

        //         let url = "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet,contentDetails&maxResults=5&playlistId={}";

        //         let response = client
        //         .get(url)
        //         .bearer_auth(token.secret())
        //         .send()
        //         .await?
        //         .error_for_status()?;

        //     })
        //     .await;
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
    etag: &mut Option<String>,
    token: &AccessToken,
) -> Result<Option<HashMap<String, ChannelMetadata>>, reqwest::Error> {
    let url = "https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=50";

    let mut channel_ids = HashMap::new();
    let mut page_token = None;

    // Pagination handling
    loop {
        let url = if let Some(page_token) = &page_token {
            Cow::Owned(format!("{url}&pageToken={page_token}"))
        } else {
            Cow::Borrowed(url)
        };

        let headers = if page_token.is_none()
            && let Some(etag) = etag
        {
            HeaderMap::from_iter([(header::IF_NONE_MATCH, HeaderValue::from_str(etag).unwrap())])
        } else {
            HeaderMap::new()
        };

        // TODO: log errors in database?
        let response = client
            .get(url.as_ref())
            .bearer_auth(token.secret())
            .headers(headers)
            .send()
            .await?
            .error_for_status()?;

        if response.status() == StatusCode::NOT_MODIFIED {
            tracing::info!("not changed");
            break Ok(None);
        }

        let json = response.json::<SubscriptionListResponse>().await?;

        if page_token.is_none() {
            // Update first etag
            *etag = Some(json.etag.unwrap());
        }

        // TODO: FIXME: so many unwrap.. Somehow better error handling

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
            break Ok(Some(channel_ids));
        }
    }
}
