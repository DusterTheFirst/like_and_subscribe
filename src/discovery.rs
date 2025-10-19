use std::{
    borrow::Cow,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use dialoguer::Input;
use eframe::egui::{
    Context,
    ahash::{HashMap, HashMapExt, HashSet},
};
use google_youtube3::{
    api::{
        ChannelListResponse, PlaylistItemListResponse, SubscriptionListResponse, VideoListResponse,
    },
    common::serde::duration,
};
use jiff::{SignedDuration, Span, Timestamp};
use oauth2::{AccessToken, ureq};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use regex::Regex;

use crate::database::Database;

mod shorts;

#[nutype::nutype(derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display))]
pub struct ChannelId(String);

#[nutype::nutype(derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display))]
pub struct PlaylistId(String);

#[nutype::nutype(derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display))]
pub struct VideoId(String);

#[nutype::nutype(derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display))]
pub struct PlaylistItemId(String);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct ChannelMetadata {
    pub name: String,
    pub profile_picture: String,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct VideoMetadata {
    pub id: VideoId,
    pub title: String,
    pub thumbnail: String,
    pub published_at: Timestamp,
    pub position: u32,
}

pub struct ChannelDiscovery {
    context: Context,
    database: Database,

    state: ChannelDiscoveryState,

    pub last_seen_video: Timestamp,
}

impl ChannelDiscovery {
    pub fn new(database: Database, context: Context) -> Self {
        Self {
            last_seen_video: database.update_date().get().unwrap_or_else(|| {
                loop {
                    match Input::<Timestamp>::new()
                        .with_prompt("Timestamp of last seen video (YYYY-MM-DDTHH:MM:SSZ)?")
                        .interact_text()
                    {
                        Ok(t) => return t,
                        Err(error) => tracing::warn!(%error, "Invalid date"),
                    }
                }
            }),

            context,
            database,

            state: ChannelDiscoveryState::Idle(Idle {}),
        }
    }

    pub fn observe_state<F: FnOnce(&mut Self, &mut ChannelDiscoveryState) -> R, R>(
        &'_ mut self,
        func: F,
    ) -> R {
        let mut new_state = match std::mem::take(&mut self.state) {
            ChannelDiscoveryState::Discovering(Discovering {
                channels: mut discovered_channels,
                mut total_channel_count,
                handle,
                incoming,
            }) => {
                while let Ok((expected, channel_id, metadata)) = incoming.try_recv() {
                    discovered_channels.insert(channel_id, metadata);
                    total_channel_count = Some(expected);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::Discovered(Discovered {
                        channels: discovered_channels,
                    })
                } else {
                    ChannelDiscoveryState::Discovering(Discovering {
                        channels: discovered_channels,
                        total_channel_count,
                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::Elaborating(Elaborating {
                channels,
                mut elaboration,
                handle,
                incoming,
            }) => {
                while let Ok((channel, metadata)) = incoming.try_recv() {
                    elaboration.insert(channel, metadata);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::Elaborated(Elaborated {
                        channels,
                        elaboration,
                    })
                } else {
                    ChannelDiscoveryState::Elaborating(Elaborating {
                        channels,
                        elaboration,

                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::FindingUploads(FindingUploads {
                channels,
                elaboration,
                mut uploads,

                handle,
                incoming,
            }) => {
                while let Ok((channel, metadata)) = incoming.try_recv() {
                    uploads.insert(channel, metadata);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::FoundUploads(FoundUploads {
                        channels,
                        elaboration,
                        uploads,
                    })
                } else {
                    ChannelDiscoveryState::FindingUploads(FindingUploads {
                        channels,
                        elaboration,
                        uploads,

                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::FilteringByShorts(FilteringByShorts {
                channels,
                elaboration,
                uploads,

                date_filtered_videos,
                mut is_short,

                handle,
                incoming,
            }) => {
                while let Ok((video, metadata)) = incoming.try_recv() {
                    is_short.insert(video, metadata);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::FilteredByShorts(FilteredByShorts {
                        channels,
                        elaboration,
                        uploads,

                        date_filtered_videos,
                        is_short,
                    })
                } else {
                    ChannelDiscoveryState::FilteringByShorts(FilteringByShorts {
                        channels,
                        elaboration,
                        uploads,

                        date_filtered_videos,
                        is_short,

                        handle,
                        incoming,
                    })
                }
            }
            state => state,
        };

        let ret = func(self, &mut new_state);

        // If no new state was inserted due to a transition, use the current next state
        if matches!(self.state, ChannelDiscoveryState::Idle(_)) {
            self.state = new_state;
        }

        ret
    }
}

pub struct Idle {}
impl Idle {
    pub fn start(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();

        let handle = std::thread::spawn(|| {
            get_all_channels(access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::Discovering(Discovering {
            channels: HashMap::new(),
            total_channel_count: None,

            incoming: rx,
            handle,
        });
    }
}

pub struct Discovering {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub total_channel_count: Option<i32>,

    handle: JoinHandle<()>,
    incoming: Receiver<(i32, ChannelId, ChannelMetadata)>,
}

pub struct Discovered {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
}
impl Discovered {
    pub fn elaborate(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let channel_ids = self.channels.keys().cloned().collect::<Vec<_>>();

        let handle = std::thread::spawn(|| {
            elaborate_all_channels(channel_ids, access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::Elaborating(Elaborating {
            channels: std::mem::take(&mut self.channels),
            elaboration: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct Elaborating {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,

    handle: JoinHandle<()>,
    incoming: Receiver<(ChannelId, PlaylistId)>,
}

pub struct Elaborated {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
}
impl Elaborated {
    pub fn find_uploads(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let playlist_ids = {
            let mut ids = self
                .elaboration
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();

            ids.sort_unstable_by_key(|(c, _)| &self.channels.get(c).unwrap().name);

            ids
        };

        let handle = std::thread::spawn(|| {
            find_recent_uploads(playlist_ids, access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::FindingUploads(FindingUploads {
            channels: std::mem::take(&mut self.channels),
            elaboration: std::mem::take(&mut self.elaboration),
            uploads: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct FindingUploads {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,

    handle: JoinHandle<()>,
    incoming: Receiver<(ChannelId, Vec<VideoMetadata>)>,
}

pub struct FoundUploads {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,
}
impl FoundUploads {
    pub fn filter(&mut self, discovery: &mut ChannelDiscovery, timestamp: Timestamp) {
        discovery.state = ChannelDiscoveryState::FilteredByDate(FilteredByDate {
            date_filtered_videos: self
                .uploads
                .iter()
                .flat_map(|(_, videos)| {
                    videos.iter().filter_map(|video| {
                        if video.published_at > timestamp {
                            Some(video.id.clone())
                        } else {
                            None
                        }
                    })
                })
                .collect(),

            channels: std::mem::take(&mut self.channels),
            elaboration: std::mem::take(&mut self.elaboration),
            uploads: std::mem::take(&mut self.uploads),
        });
    }
}

pub struct FilteredByDate {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,

    pub date_filtered_videos: HashSet<VideoId>,
}
impl FilteredByDate {
    pub fn filter(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let video_ids = self
            .date_filtered_videos
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        let handle = std::thread::spawn(|| {
            determine_is_short(video_ids, access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::FilteringByShorts(FilteringByShorts {
            channels: std::mem::take(&mut self.channels),
            elaboration: std::mem::take(&mut self.elaboration),
            uploads: std::mem::take(&mut self.uploads),

            date_filtered_videos: std::mem::take(&mut self.date_filtered_videos),
            is_short: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct FilteringByShorts {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,

    pub date_filtered_videos: HashSet<VideoId>,
    pub is_short: HashMap<VideoId, bool>,

    handle: JoinHandle<()>,
    incoming: Receiver<(VideoId, bool)>,
}

pub struct FilteredByShorts {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,

    pub date_filtered_videos: HashSet<VideoId>,
    pub is_short: HashMap<VideoId, bool>,
}
// pub struct FilteredByPlaylist {}

pub enum ChannelDiscoveryState {
    Idle(Idle),

    Discovering(Discovering),
    Discovered(Discovered),

    Elaborating(Elaborating),
    Elaborated(Elaborated),

    FindingUploads(FindingUploads),
    FoundUploads(FoundUploads),

    FilteredByDate(FilteredByDate),

    FilteringByShorts(FilteringByShorts),
    FilteredByShorts(FilteredByShorts),
}

impl Default for ChannelDiscoveryState {
    fn default() -> Self {
        ChannelDiscoveryState::Idle(Idle {})
    }
}

// TODO: etag and caching
fn get_all_channels(
    token: AccessToken,
    channel: Sender<(i32, ChannelId, ChannelMetadata)>,
    context: Context,
) {
    let url = "https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=50";

    let mut page_token = None;

    // Pagination handling
    loop {
        let url = if let Some(page_token) = &page_token {
            Cow::Owned(format!("{url}&pageToken={page_token}"))
        } else {
            Cow::Borrowed(url)
        };

        // TODO: log errors in database?
        let json = ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
            .unwrap()
            .into_json::<SubscriptionListResponse>()
            .unwrap();

        // TODO: FIXME: so many unwrap.. Somehow better error handling

        let total_results = json.page_info.unwrap().total_results.unwrap();
        let items = json.items.unwrap();

        for subscription in items {
            let snippet = subscription.snippet.unwrap();
            let resource = snippet.resource_id.unwrap();

            debug_assert_eq!(resource.kind.as_deref(), Some("youtube#channel"));

            let channel_id = ChannelId::new(resource.channel_id.unwrap());
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

            channel
                .send((
                    total_results,
                    channel_id,
                    ChannelMetadata {
                        name: channel_name,
                        profile_picture: channel_thumbnail.url.unwrap(),
                    },
                ))
                .unwrap();
        }
        tracing::info!("page");
        context.request_repaint();

        page_token = json.next_page_token;

        if page_token.is_none() {
            break;
        }
    }
}

// Playlists are always -> channel_id.replacen("UC", "UU", 1);
fn elaborate_all_channels(
    channel_ids: Vec<ChannelId>,
    token: AccessToken,
    channel: Sender<(ChannelId, PlaylistId)>,
    context: Context,
) {
    for chunk in channel_ids.chunks(50) {
        let url =
            "https://www.googleapis.com/youtube/v3/channels?part=id,contentDetails&maxResults=50";
        let url = format!(
            "{url}&id={}",
            chunk
                .iter()
                .cloned()
                .map(ChannelId::into_inner)
                .collect::<Vec<_>>()
                .join(",")
        );

        let json = ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
            .unwrap()
            .into_json::<ChannelListResponse>()
            .unwrap();

        let items = json.items.unwrap();

        for item in items {
            channel
                .send((
                    ChannelId::new(item.id.unwrap()),
                    PlaylistId::new(
                        item.content_details
                            .unwrap()
                            .related_playlists
                            .unwrap()
                            .uploads
                            .unwrap(),
                    ),
                ))
                .unwrap();
        }
        tracing::info!("chunk");
        context.request_repaint();
    }
}

fn find_recent_uploads(
    playlist_ids: Vec<(ChannelId, PlaylistId)>,
    token: AccessToken,
    channel: Sender<(ChannelId, Vec<VideoMetadata>)>,
    context: Context,
) {
    playlist_ids.into_par_iter().for_each(|(channel_id, playlist_id)| {
        let url = "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet,contentDetails&maxResults=50";
        let url = format!("{url}&playlistId={playlist_id}");

        let json = match ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
        {
            Ok(response) => response.into_json::<PlaylistItemListResponse>().unwrap(),
            Err(ureq::Error::Status(404, _)) => {
                // TODO: report this
                return
            },
            Err(error) => {
                panic!("{}", error)
            }
        };

        let items = json.items.unwrap();

        let videos = items
            .into_iter()
            .map(|item| {
                let snippet = item.snippet.unwrap();
                let content_details = item.content_details.unwrap();

                let position = snippet.position.unwrap();
                let title = snippet.title.unwrap();
                let thumbnail = snippet.thumbnails.unwrap().default.unwrap();

                let video_id = VideoId::new(content_details.video_id.unwrap());
                let published_at = Timestamp::from_millisecond(
                    content_details
                        .video_published_at
                        .unwrap()
                        .timestamp_millis(),
                )
                .unwrap();

                VideoMetadata {
                    id: video_id,
                    title,
                    thumbnail: thumbnail.url.unwrap(),
                    published_at,
                    position,
                }
            })
            .collect::<Vec<_>>();

        tracing::info!(%channel_id, "channel");
        channel.send((channel_id, videos)).unwrap();
        context.request_repaint();
    });
}

fn determine_is_short(
    video_ids: Vec<VideoId>,
    token: AccessToken,
    channel: Sender<(VideoId, bool)>,
    context: Context,
) {
    let embed_regex = Regex::new(r#"width="(\d+)"\s+height="(\d+)""#).unwrap();

    for videos in video_ids.chunks(50) {
        let url = "https://www.googleapis.com/youtube/v3/videos?part=contentDetails,player,snippet&maxResults=50";
        let url = format!(
            "{url}&id={}",
            videos
                .iter()
                .cloned()
                .map(VideoId::into_inner)
                .collect::<Vec<_>>()
                .join(",")
        );

        let json = ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
            .unwrap()
            .into_json::<VideoListResponse>()
            .unwrap();

        let videos = match json.items {
            Some(items) => items,
            None => {
                // TODO: Report
                tracing::warn!("no videos returned");
                return;
            }
        };

        for video in videos {
            let video_id = video.id.unwrap();

            let is_livestream = video.snippet.unwrap().live_broadcast_content.unwrap() != "none";

            if is_livestream {
                tracing::trace!(video_id, "livestream");
                continue;
            }

            let duration = video.content_details.unwrap().duration.unwrap();
            let duration = duration.parse::<SignedDuration>().unwrap_or_else(|error| {
                panic!(
                    "Failed to parse duration \"{duration}\" of video {video_id} due to: {error}"
                )
            });

            let is_duration_ok = duration <= SignedDuration::from_mins(3);

            let embed_html = video.player.unwrap().embed_html.unwrap();
            let Some(captures) = embed_regex.captures(&embed_html) else {
                tracing::error!(%embed_html, ?embed_regex, "regex failed to match embed html");
                continue;
            };
            let is_aspect_ratio_ok = {
                let width: u32 = captures[1].parse().unwrap();
                let height: u32 = captures[2].parse().unwrap();
                height >= width && width > 0
            };

            let is_short = is_duration_ok && is_aspect_ratio_ok;

            channel.send((VideoId::new(video_id), is_short)).unwrap();
        }
        tracing::info!("Processed a chunk of videos for Shorts detection.");
        context.request_repaint();
    }
}
