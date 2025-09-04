use std::{
    collections::{HashMap, hash_map::Entry},
    pin::pin,
    sync::{Arc, Mutex},
};

use futures::{StreamExt, stream};
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemSnippet, ResourceId},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use jiff::Unit;
use tokio::{select, sync::mpsc::Receiver};
use tracing::{Instrument, debug, error, trace, warn};

use crate::{
    feed::Feed, playlist::shorts::check_redirect, subscription::YoutubeChannelSubscription,
};

mod shorts;

pub async fn youtube_playlist_modifier(
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    client: reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    subscriptions: Arc<Mutex<HashMap<String, YoutubeChannelSubscription>>>,
    playlist_id: Arc<str>,
    mut reciever: Receiver<(tracing::Span, Feed)>,
) {
    let stream_processing = stream::poll_fn(|cx| reciever.poll_recv(cx)).for_each_concurrent(
        10,
        |(span, Feed { entry, .. })| {
            let youtube = youtube.clone();
            let client = client.clone();
            let subscriptions = subscriptions.as_ref();
            let playlist_id = playlist_id.as_ref();
            let span2 = span.clone();

            async move {
                if entry.video_id == "BxV14h0kFs0" {
                    trace!("skipping tom scott automated video");
                    return;
                }

                trace!("validating new feed item");

                match subscriptions
                    .lock()
                    .expect("mutex should not be poisoned")
                    .entry(entry.channel_id.clone())
                {
                    Entry::Occupied(occupied_entry) => {
                        span.record("channel_name", &occupied_entry.get().name);
                    }
                    Entry::Vacant(vacant_entry) => {
                        // Queue for unsubscription
                        // TODO: just unsubscribe here?
                        vacant_entry.insert(YoutubeChannelSubscription {
                            name: String::new(),
                            subscription_expiration: None,
                            stale: true,
                        });
                        warn!(
                            channel_id = entry.channel_id,
                            "feed item had unknown channel"
                        );
                        return;
                    }
                };

                let video_age_minutes = (entry.updated - entry.published)
                    .total((Unit::Minute, entry.updated))
                    .expect("span arithmatic should not overflow");

                span.record("video_age_minutes", video_age_minutes);

                if video_age_minutes > 1.0 {
                    debug!("ignoring updated old video");
                    return;
                }

                // Check if the video is a short
                let is_short_future = async {
                    // TODO: do something with the reason?
                    // Do not flag as a short if we are not sure
                    let is_short = check_redirect(&entry.video_id, &client).await;

                    match is_short.as_ref() {
                        Ok(false) => false,
                        Ok(true) => {
                            tracing::debug!("video is a short");
                            true
                        }
                        Err(error) => {
                            tracing::warn!(?error, "unable to determine if video is a short");
                            false
                        }
                    }
                };

                // Duplicate detection
                let detect_duplicate = async {
                    let result = youtube
                        .playlist_items()
                        .list(&vec!["contentDetails".to_string()])
                        .playlist_id(playlist_id)
                        .video_id(&entry.video_id)
                        .doit()
                        .await;

                    match result {
                        Ok((_, items)) => {
                            let item_exists = items.items.into_iter().flatten().any(|i| {
                                i.content_details.as_ref().and_then(|d| d.video_id.as_ref())
                                    == Some(&entry.video_id)
                            });

                            if item_exists {
                                warn!("video exists in playlist already, skipping");
                                return true;
                            }
                        }
                        Err(error) => {
                            warn!(%error, "failed to check if video exists in playlist already");
                        }
                    }

                    false
                };

                let mut is_short_future = pin!(is_short_future);
                let mut detect_duplicate = pin!(detect_duplicate);

                // Concurrent short circuiting || (or)
                let skip = select! {
                    is_short = &mut is_short_future => {
                        if is_short {
                            true
                        } else {
                            detect_duplicate.await
                        }
                    }
                    is_duplicate = &mut detect_duplicate => {
                        if is_duplicate {
                            true
                        } else {
                            is_short_future.await
                        }
                    }
                };

                if skip {
                    return;
                }

                trace!("inserting new video");
                let result = youtube
                    .playlist_items()
                    .insert(PlaylistItem {
                        snippet: Some(PlaylistItemSnippet {
                            playlist_id: Some(playlist_id.to_string()),
                            resource_id: Some(ResourceId {
                                kind: Some("youtube#video".into()),
                                video_id: Some(entry.video_id.clone()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .doit()
                    .await;

                match result {
                    Ok(_) => {
                        debug!("video inserted");
                        span.record("inserted", true);
                    }
                    Err(error) => {
                        error!(%error, "failed to insert video");
                    }
                }
            }
            .instrument(span2)
        },
    );

    tokio::select! {
        _  = stream_processing => tracing::info!("stream processing task exited"),
        _ = shutdown.recv() => tracing::info!("stream processing task shutting down"),
    };
}
