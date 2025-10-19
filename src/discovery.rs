use std::{
    borrow::Cow,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use dialoguer::Input;
use eframe::egui::{
    Context,
    ahash::{HashMap, HashMapExt, HashSet, HashSetExt as _},
};
use google_youtube3::api::{PlaylistItemListResponse, SubscriptionListResponse};
use jiff::Timestamp;
use oauth2::{AccessToken, ureq};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::database::Database;

#[nutype::nutype(
    derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display, AsRef, Borrow, FromStr),
    validate(predicate = |str| str.starts_with("UC"))
)]
pub struct ChannelId(String);

impl ChannelId {
    fn internal_id(&self) -> &str {
        self.as_ref().strip_prefix("UC").unwrap()
    }

    #[expect(dead_code)]
    pub fn playlist_uploads(&self) -> PlaylistId {
        PlaylistId::new(format!("UU{}", self.internal_id()))
    }

    #[expect(dead_code)]
    pub fn playlist_shorts(&self) -> PlaylistId {
        PlaylistId::new(format!("UUSH{}", self.internal_id()))
    }

    pub fn playlist_long_form(&self) -> PlaylistId {
        PlaylistId::new(format!("UULF{}", self.internal_id()))
    }

    #[expect(dead_code)]
    pub fn playlist_livestream(&self) -> PlaylistId {
        PlaylistId::new(format!("UULV{}", self.internal_id()))
    }
}

#[nutype::nutype(derive(
    Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display, AsRef, Borrow, FromStr
))]
pub struct PlaylistId(String);

#[nutype::nutype(derive(
    Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Display, AsRef, Borrow
))]
pub struct VideoId(String);

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

    pub playlist_id: PlaylistId,
    pub last_seen_video: Timestamp,
}

impl ChannelDiscovery {
    pub fn new(database: Database, playlist_id: PlaylistId, context: Context) -> Self {
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

            playlist_id,
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
            ChannelDiscoveryState::FindingUploads(FindingUploads {
                channels,
                mut uploads,

                handle,
                incoming,
            }) => {
                while let Ok((channel, metadata)) = incoming.try_recv() {
                    uploads
                        .entry(channel)
                        .or_default()
                        .extend_from_slice(metadata.as_slice());
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::FoundUploads(FoundUploads { channels, uploads })
                } else {
                    ChannelDiscoveryState::FindingUploads(FindingUploads {
                        channels,
                        uploads,

                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::FindingPlaylistItems(FindingPlaylistItems {
                channels,
                uploads,

                mut playlist_items,
                mut total_playlist_items,

                handle,
                incoming,
            }) => {
                while let Ok((total, videos)) = incoming.try_recv() {
                    total_playlist_items = Some(total);
                    playlist_items.extend(videos);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::FoundPlaylistItems(FoundPlaylistItems {
                        channels,
                        uploads,
                        playlist_items,
                    })
                } else {
                    ChannelDiscoveryState::FindingPlaylistItems(FindingPlaylistItems {
                        channels,
                        uploads,

                        playlist_items,
                        total_playlist_items,

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
    pub fn find_uploads(
        &mut self,
        discovery: &mut ChannelDiscovery,
        until: Timestamp,
        access_token: AccessToken,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let playlist_ids = self
            .channels
            .keys()
            .map(|k| (k.clone(), k.playlist_long_form()))
            .collect::<Vec<_>>();

        let handle = std::thread::spawn(move || {
            find_recent_uploads(playlist_ids, access_token, until, tx, context);
        });

        discovery.state = ChannelDiscoveryState::FindingUploads(FindingUploads {
            channels: std::mem::take(&mut self.channels),
            uploads: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct FindingUploads {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,

    handle: JoinHandle<()>,
    incoming: Receiver<(ChannelId, Vec<VideoMetadata>)>,
}

pub struct FoundUploads {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,
}

impl FoundUploads {
    pub fn find_playlist_items(
        &mut self,
        discovery: &mut ChannelDiscovery,
        access_token: AccessToken,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let playlist = discovery.playlist_id.clone();

        let handle = std::thread::spawn(move || {
            find_playlist_items(playlist, access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::FindingPlaylistItems(FindingPlaylistItems {
            channels: std::mem::take(&mut self.channels),
            uploads: std::mem::take(&mut self.uploads),
            playlist_items: HashSet::new(),
            total_playlist_items: None,

            incoming: rx,
            handle,
        });
    }
}

pub struct FindingPlaylistItems {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,
    pub playlist_items: HashSet<VideoId>,
    pub total_playlist_items: Option<i32>,

    handle: JoinHandle<()>,
    incoming: Receiver<(i32, Vec<VideoId>)>,
}

pub struct FoundPlaylistItems {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,
    pub playlist_items: HashSet<VideoId>,
}

pub enum ChannelDiscoveryState {
    Idle(Idle),

    Discovering(Discovering),
    Discovered(Discovered),

    FindingUploads(FindingUploads),
    FoundUploads(FoundUploads),

    FindingPlaylistItems(FindingPlaylistItems),
    FoundPlaylistItems(FoundPlaylistItems),
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

            let channel_id = ChannelId::try_new(resource.channel_id.unwrap()).unwrap();
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

fn find_recent_uploads(
    playlist_ids: Vec<(ChannelId, PlaylistId)>,
    token: AccessToken,
    until: Timestamp,
    channel: Sender<(ChannelId, Vec<VideoMetadata>)>,
    context: Context,
) {
    playlist_ids.into_par_iter().for_each(|(channel_id, playlist_id)| {
        let base_url = format!("https://www.googleapis.com/youtube/v3/playlistItems?part=snippet,contentDetails&maxResults=50&playlistId={playlist_id}");
        let mut page_token: Option<String> = None;

        loop {
            let url = if let Some(token) = &page_token {
                Cow::Owned(format!("{base_url}&pageToken={token}"))
            } else {
                Cow::Borrowed(&base_url)
            };

            let json = match ureq::get(url.as_ref())
                .set("Authorization", &format!("Bearer {}", token.secret()))
                .call()
            {
                Ok(response) => response.into_json::<PlaylistItemListResponse>().unwrap(),
                Err(ureq::Error::Status(404, _)) => {
                    // TODO: report this
                    break;
                },
                Err(error) => {
                    panic!("{}", error)
                }
            };

            let items = json.items.unwrap_or_default();
            let mut should_break = false;

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

                    if published_at <= until {
                        should_break = true;
                    }

                    VideoMetadata {
                        id: video_id,
                        title,
                        thumbnail: thumbnail.url.unwrap(),
                        published_at,
                        position,
                    }
                })
                .collect::<Vec<_>>();

            channel.send((channel_id.clone(), videos)).unwrap();
            context.request_repaint();

            page_token = json.next_page_token;
            if page_token.is_none() || should_break {
                break;
            }
        }
        tracing::info!(%channel_id, "channel");

    });
}

fn find_playlist_items(
    playlist_id: PlaylistId,
    token: AccessToken,
    channel: Sender<(i32, Vec<VideoId>)>,
    context: Context,
) {
    let base_url = format!(
        "https://www.googleapis.com/youtube/v3/playlistItems?part=contentDetails&maxResults=50&playlistId={playlist_id}"
    );
    let mut page_token: Option<String> = None;

    loop {
        let url = if let Some(token) = &page_token {
            Cow::Owned(format!("{base_url}&pageToken={token}"))
        } else {
            Cow::Borrowed(&base_url)
        };

        let json = match ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
        {
            Ok(response) => response.into_json::<PlaylistItemListResponse>().unwrap(),
            Err(ureq::Error::Status(404, _)) => {
                // TODO: report this error, maybe through a different channel or by logging to the database.
                tracing::warn!(%playlist_id, "Playlist not found (404)");
                break;
            }
            Err(error) => {
                // TODO: Better error handling than panic.
                panic!("Failed to fetch playlist items: {}", error);
            }
        };

        let items = json.items.unwrap_or_default();
        let total_results = json.page_info.unwrap().total_results.unwrap();

        let video_ids = items
            .into_iter()
            .filter_map(|item| item.content_details?.video_id)
            .map(VideoId::new)
            .collect::<Vec<_>>();

        channel.send((total_results, video_ids)).unwrap();
        context.request_repaint();
        tracing::info!("page");

        page_token = json.next_page_token;
        if page_token.is_none() {
            break;
        }
    }
}
