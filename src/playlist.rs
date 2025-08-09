use std::{
    cmp::Ordering,
    collections::{HashMap, hash_map::Entry},
    pin::pin,
    str::FromStr,
    sync::{Arc, Mutex},
};

use bstr::ByteSlice;
use futures::{StreamExt, stream};
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemSnippet, ResourceId},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use jiff::Unit;
use reqwest::header;
use tokio::{select, sync::mpsc::Receiver};
use tracing::{Instrument, debug, error, trace, warn};

use crate::{feed::Feed, subscription::YoutubeChannelSubscription};

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
                    debug!("ignoring updated old video");
                    return;
                }

                // Check if the video is a short


                let is_short_future = async {
                    #[derive(Debug)]
                    enum ShortsScore {
                        Indeterminate(ShortsIndeterminateReason),
                        Determinate(bool),
                        Heuristic {
                            duration: bool,
                            vertical: bool,
                            hashtag: bool
                        },
                    }

                    #[derive(Debug)]
                    enum ShortsIndeterminateReason {
                        BadRequest,
                        BadResponse,
                        NonWatchRedirect,
                    }

                    let check_redirect = async {
                        let result = client
                            .execute(
                                client
                                    .head(format!("https://www.youtube.com/shorts/{}", entry.video_id))
                                    .build()
                                    .unwrap(),
                            )
                            .await;

                            let response = match result {
                                Ok(response) => response,
                                Err(error) => {
                                    warn!(%error, "failed to request shorts url");
                                    return ShortsScore::Indeterminate(
                                        ShortsIndeterminateReason::BadRequest,
                                    );
                                }
                            };

                            if response.status().is_success() {
                                ShortsScore::Determinate(true)
                            } else if response.status().is_redirection() {
                                let Some(location) = response.headers().get(header::LOCATION) else {
                                    error!(
                                        ?response,
                                        "redirect response did not contain a Location header"
                                    );
                                    return ShortsScore::Indeterminate(
                                        ShortsIndeterminateReason::BadResponse,
                                    );
                                };

                                if location.as_bytes().contains_str("watch") {
                                    ShortsScore::Determinate(false)
                                } else {
                                    ShortsScore::Indeterminate(
                                        ShortsIndeterminateReason::NonWatchRedirect,
                                    )
                                }
                            } else {
                                error!(?response, "redirect response had unexpected status code");
                                ShortsScore::Indeterminate(ShortsIndeterminateReason::BadResponse)
                            }
                        };

                        let check_metadata = async {
                            let result = youtube
                                .videos()
                                .list(&vec!["contentDetails".into(), "snippet".into()])
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

                                    let duration_heuristic = 'duration :{
                                        let duration = video.content_details.as_ref().and_then(|d| d.duration.as_deref());
                                        let Some(duration) = duration else {
                                            warn!(?video, "unable to extract iso duration from video");
                                            break 'duration false;
                                        };

                                        let duration = match jiff::Span::from_str(duration) {
                                            Ok(duration) => duration,
                                            Err(error) => {
                                                error!(%error, %duration, "unable to parse duration");
                                                break 'duration false;
                                            },
                                        };

                                        let ordering = match duration.compare(jiff::Span::new().seconds(180)) {
                                            Ok(ordering) => ordering,
                                            Err(error) => {
                                                error!(%error, %duration, "unable to compare video duration");
                                                break 'duration false;
                                            },
                                        };

                                        match ordering {
                                            Ordering::Less | Ordering::Equal  => true,
                                            Ordering::Greater => false,
                                        }
                                    };

                                    let hashtag_heuristic = 'hashtag: {
                                        let title = video.snippet.as_ref().and_then(|s| Option::zip(s.title.as_deref(), s.description.as_deref()));
                                        let Some((title, description)) = title else {
                                            warn!(?video, "unable to extract title and description from video");
                                            break 'hashtag false;
                                        };

                                        let pattern = "#shorts";

                                        title.contains(pattern) || description.contains(pattern)
                                    };

                                    let vertical_heuristic = 'vertical: {
                                        let dimensions = video
                                            .snippet.as_ref()
                                            .and_then(|s| s.thumbnails.as_ref())
                                            .and_then(|t| {
                                                t.default.as_ref()
                                                    .or(t.standard.as_ref())
                                                    .or(t.medium.as_ref())
                                                    .or(t.high.as_ref())
                                                    .or(t.maxres.as_ref())
                                            })
                                            .and_then(|d| Option::zip(d.height, d.width));

                                        let Some((height, width)) = dimensions else {
                                                warn!(?video, "unable to extract thumbnail sizes");

                                            break 'vertical false;
                                        };

                                        height > width
                                    };

                                    ShortsScore::Heuristic { duration: duration_heuristic, vertical: vertical_heuristic, hashtag: hashtag_heuristic }
                                }
                                Err(error) => {
                                    warn!(%error, "failed to get video metadata");
                                    ShortsScore::Indeterminate(
                                        ShortsIndeterminateReason::BadResponse,
                                    )
                                }
                            }
                        };

                    let mut check_redirect = pin!(check_redirect);
                    let mut check_metadata = pin!(check_metadata);

                    let score = select! {
                        score = &mut check_redirect => {
                            if matches!(score, ShortsScore::Indeterminate(_)) {
                                check_metadata.await
                            } else {
                                score
                            }
                        }
                        score = &mut check_metadata => {
                            if matches!(score, ShortsScore::Indeterminate(_)) {
                                check_redirect.await
                            } else {
                                score
                            }
                        }
                    };

                    span.record("short_score", format!("{score:?}"));

                    match score {
                        ShortsScore::Determinate(result) => result,
                        ShortsScore::Heuristic { duration, vertical, hashtag } => {
                            // Heuristic decision
                            duration && (vertical || hashtag)
                        }
                        ShortsScore::Indeterminate(shorts_indeterminate_reason) => {
                            // TODO: do something with the reason?
                            // Do not flag as a short if we are not sure
                            false
                        },
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
