use std::{
    borrow::Cow,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use eframe::egui::{
    Context,
    ahash::{HashMap, HashMapExt},
};
use google_youtube3::api::{
    ChannelListResponse, PlaylistItemListResponse, SubscriptionListResponse,
};
use jiff::Timestamp;
use oauth2::{AccessToken, ureq};

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
    state: ChannelDiscoveryState,
}

impl ChannelDiscovery {
    pub fn new(context: Context) -> Self {
        Self {
            context,
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

                mut current_channel,

                handle,
                incoming,
            }) => {
                while let Ok((channel, metadata)) = incoming.try_recv() {
                    uploads.insert(channel.clone(), metadata);
                    current_channel = Some(channel);
                }

                if handle.is_finished() {
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

                        current_channel,

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

            current_channel: None,

            incoming: rx,
            handle,
        });
    }
}

pub struct FindingUploads {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,

    pub current_channel: Option<ChannelId>,

    handle: JoinHandle<()>,
    incoming: Receiver<(ChannelId, Vec<VideoMetadata>)>,
}

pub struct FoundUploads {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub elaboration: HashMap<ChannelId, PlaylistId>,
    pub uploads: HashMap<ChannelId, Vec<VideoMetadata>>,
}

pub enum ChannelDiscoveryState {
    Idle(Idle),
    Discovering(Discovering),
    Discovered(Discovered),
    Elaborating(Elaborating),
    Elaborated(Elaborated),
    FindingUploads(FindingUploads),
    FoundUploads(FoundUploads),
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
    for (channel_id, playlist_id) in playlist_ids {
        let url = "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet,contentDetails&maxResults=50";
        let url = format!("{url}&playlistId={playlist_id}");

        let json = match ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
        {
            Ok(response) => response.into_json::<PlaylistItemListResponse>().unwrap(),
            Err(ureq::Error::Status(404, _)) => continue,
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
    }
}
