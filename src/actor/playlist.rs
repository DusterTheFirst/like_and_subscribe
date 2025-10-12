use std::{borrow::Cow, collections::HashMap, error::Error, sync::Arc, time::Duration};

use axum::http::{HeaderMap, HeaderValue};
use google_youtube3::api::{
    PlaylistItem, PlaylistItemListResponse, PlaylistItemSnippet, ResourceId,
    SubscriptionListResponse,
};
use jiff::Timestamp;
use oauth2::AccessToken;
use reqwest::{StatusCode, header};
use tokio_util::sync::CancellationToken;

use crate::{actor::playlist::shorts::check_redirect, database::Database, oauth::TokenManager};

mod shorts;

pub async fn playlist_updater(
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

        for (channel_id, channel_info) in &subscribed_channels {
            let last_update = database.update_date().get(channel_id.clone()).await?;

            tracing::debug!(channel_id, channel_name = channel_info.name, "checking");

            let result = async {
                // FIXME: this might be fragile, actually use the API?
                // Also, some channels don't have a playlist
                let channel_playlist_id = channel_id.replacen("UC", "UU", 1);

                // Channel playlist
                let url = "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet,contentDetails&maxResults=50";
                let url = format!("{url}&playlistId={channel_playlist_id}");

                // TODO: API interface
                let response = client
                    .get(url)
                    .bearer_auth(token.secret())
                    .send()
                    .await?
                    .error_for_status()?;

                let json: PlaylistItemListResponse = response.json().await?;

                for item in json.items.unwrap() {
                    let snippet = item.snippet.unwrap();
                    let video_id = snippet.resource_id.unwrap().video_id.unwrap();

                    let published_at = Timestamp::from_millisecond(snippet.published_at.as_ref().unwrap().timestamp_millis()).unwrap();

                    if published_at.duration_until(last_update).is_positive() {
                        tracing::debug!(%published_at, %last_update, video_id, channel_id, "video too old, done");
                        // Old
                        break;
                    }

                    // subscriptions playlist
                    let url = "https://www.googleapis.com/youtube/v3/playlistItems";
                    let url = format!("{url}?playlistId={playlist_id}&videoId={}", video_id);

                    let response = client
                        .get(url)
                        .bearer_auth(token.secret())
                        .send()
                        .await?
                        .error_for_status()?;

                    let json: PlaylistItemListResponse = response.json().await?;

                    let results_count = json.page_info.as_ref().unwrap().total_results.unwrap();

                    if results_count != 0 {
                        tracing::debug!(%published_at, %last_update, video_id, channel_id, "exists already");
                        database.update_date().set(channel_id.clone(), published_at).await?;
                        // Video already added
                        continue;
                    }

                    // Check if the video is a short
                    // TODO: do something with the reason?
                    // Do not flag as a short if we are not sure
                    match check_redirect(&video_id, &client).await {
                        Ok(false) => {},
                        Ok(true) => {
                            tracing::debug!("video is a short");
                            continue;
                        }
                        Err(error) => {
                            tracing::warn!(?error, "unable to determine if video is a short");
                        }
                    }

                    // subscriptions playlist
                    let url = "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet";
                    let url = format!("{url}&playlistId={playlist_id}");

                    // TODO: API interface
                    // TODO: keep track of inserted playlist item
                    client
                        .post(url)
                        .bearer_auth(token.secret()).json(&PlaylistItem {
                            snippet: Some(PlaylistItemSnippet {
                                playlist_id: Some(playlist_id.to_string()),
                                resource_id: Some(ResourceId { // TODO: resource id should be an enum with a tag
                                    kind: Some(String::from("youtube#video")),
                                    video_id: Some(video_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        } )
                        .send()
                        .await?
                        .error_for_status()?;

                    database.update_date().set(channel_id.clone(), published_at).await?;
                }

                Ok::<_, Box<dyn Error>>(())
            }
            .await;

            if let Err(error) = result {
                //TODO: database?
                tracing::error!(%error, %channel_id, channel_name=%channel_info.name, "failed to handle channel");
            }
        }

        tracing::info!("update complete");
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
