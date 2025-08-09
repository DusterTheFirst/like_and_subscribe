use std::{
    collections::{HashMap, hash_map::Entry},
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
use tokio::sync::mpsc::Receiver;
use tracing::{Instrument, debug, error, trace, warn};

use crate::{feed::Feed, subscription::YoutubeChannelSubscription};

pub async fn youtube_playlist_modifier(
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    subscriptions: Arc<Mutex<HashMap<String, YoutubeChannelSubscription>>>,
    playlist_id: Arc<str>,
    mut reciever: Receiver<(tracing::Span, Feed)>,
) {
    let stream_processing = stream::poll_fn(|cx| reciever.poll_recv(cx)).for_each_concurrent(
        10,
        |(span, Feed { entry, .. })| {
            let youtube = youtube.clone();
            let subscriptions = subscriptions.as_ref();
            let playlist_id = playlist_id.as_ref();
            let span2 = span.clone();

            async move {
                if entry.video_id == "BxV14h0kFs0" {
                    trace!("skipping tom scott automated video");
                    return;
                }

                debug!("validating new feed item");

                match subscriptions
                    .lock()
                    .unwrap()
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
                    .unwrap();

                span.record("video_age_minutes", video_age_minutes);

                if video_age_minutes > 1.0 {
                    trace!("ignoring updated old video");
                    span.record("inserted", false);
                    return;
                }

                // TODO: shorts detection: https://issuetracker.google.com/issues/232112727
                // Check if the video is a short
                let result = youtube
                    .videos()
                    .list(&vec!["contentDetails".into(), "player".into()])
                    .add_id(&entry.video_id)
                    .doit()
                    .await;

                match result {
                    Ok((_, items)) => {
                        let video = items
                            .items
                            .iter()
                            .flatten()
                            .next()
                            .expect("exactly one entry should be returned");

                        let embed = video.player.as_ref().unwrap().embed_html.as_ref().unwrap();
                        let is_short = embed.contains("youtube.com/shorts/");

                        if is_short {
                            debug!(%embed, "ignoring shorts video");
                            span.record("inserted", false);
                            return;
                        }
                    }
                    Err(error) => {
                        warn!(%error, "failed to check if video is a short");
                    }
                }

                span.record("inserted", true);

                // Duplicate detection
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
                            return;
                        }
                    }
                    Err(error) => {
                        warn!(%error, "failed to check if video exists in playlist already");
                    }
                }

                debug!("inserting new video");

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
