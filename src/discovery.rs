use std::{
    borrow::Cow,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use eframe::egui::{
    Context,
    ahash::{HashMap, HashMapExt, HashSet, HashSetExt as _},
};
use google_youtube3::api::{
    Playlist, PlaylistItem, PlaylistItemListResponse, PlaylistItemSnippet, PlaylistListResponse,
    PlaylistSnippet, ResourceId, SubscriptionListResponse, VideoListResponse,
};
use jiff::Timestamp;
use oauth2::{AccessToken, ureq};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

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
    pub title: String,
    pub thumbnail: String,
    pub published_at: Timestamp,
    pub position: u32,
}

pub struct ChannelDiscovery {
    context: Context,

    state: ChannelDiscoveryState,

    pub playlist_id: PlaylistId,
}

impl ChannelDiscovery {
    pub fn new(playlist_id: PlaylistId, context: Context) -> Self {
        Self {
            context,

            playlist_id,

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
                    let last_update = handle.join().unwrap();
                    ChannelDiscoveryState::Discovered(Discovered {
                        last_update,

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
                last_update,

                channels,
                mut videos,

                mut uploads,

                handle,
                incoming,
            }) => {
                while let Ok((channel, new_videos)) = incoming.try_recv() {
                    uploads
                        .entry(channel)
                        .or_default()
                        .extend(new_videos.iter().map(|(id, _)| id.clone()));

                    videos.extend(new_videos);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::FoundUploads(FoundUploads {
                        last_update,

                        channels,
                        videos,
                        uploads,
                    })
                } else {
                    ChannelDiscoveryState::FindingUploads(FindingUploads {
                        last_update,
                        channels,
                        videos,

                        uploads,

                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::FindingPlaylistItems(FindingPlaylistItems {
                channels,
                videos,

                uploads,
                new_videos,

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
                        videos,

                        uploads,
                        new_videos,
                        playlist_items,
                    })
                } else {
                    ChannelDiscoveryState::FindingPlaylistItems(FindingPlaylistItems {
                        channels,
                        videos,

                        uploads,
                        new_videos,

                        playlist_items,
                        total_playlist_items,

                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::DeterminingLanguages(DeterminingLanguages {
                channels,
                videos,

                uploads,
                new_videos,
                playlist_items,

                mut video_languages,

                handle,
                incoming,
            }) => {
                while let Ok((video, language)) = incoming.try_recv() {
                    video_languages.insert(video, language);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::DeterminedLanguages(DeterminedLanguages {
                        // TODO: move somewhere else
                        excluded_languages: HashSet::from_iter(
                            video_languages
                                .values()
                                .collect::<HashSet<_>>()
                                .into_iter()
                                .filter(|l| !l.starts_with("en"))
                                .cloned(),
                        ),

                        channels,
                        videos,

                        uploads,
                        video_languages,

                        new_videos,
                        playlist_items,
                    })
                } else {
                    ChannelDiscoveryState::DeterminingLanguages(DeterminingLanguages {
                        channels,
                        videos,

                        uploads,
                        video_languages,

                        new_videos,
                        playlist_items,

                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::AddingToPlaylist(AddingToPlaylist {
                channels,
                videos,

                uploads,
                new_videos,
                mut playlist_items,
                excluded_languages,

                video_languages,

                handle,
                incoming,
            }) => {
                while let Ok(video) = incoming.try_recv() {
                    playlist_items.insert(video);
                }

                if handle.is_finished() {
                    handle.join().unwrap();
                    ChannelDiscoveryState::Done(Done {
                        channels,
                        videos,

                        uploads,
                        video_languages,

                        new_videos,
                        playlist_items,
                    })
                } else {
                    ChannelDiscoveryState::AddingToPlaylist(AddingToPlaylist {
                        channels,
                        videos,

                        uploads,
                        video_languages,

                        new_videos,
                        playlist_items,
                        excluded_languages,

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
        let playlist_id = discovery.playlist_id.clone();

        let handle = std::thread::spawn(move || {
            get_all_channels(&access_token, tx, context);

            get_playlist_metadata(playlist_id, &access_token)
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

    handle: JoinHandle<Timestamp>,
    incoming: Receiver<(i32, ChannelId, ChannelMetadata)>,
}

pub struct Discovered {
    pub last_update: Timestamp,

    pub channels: HashMap<ChannelId, ChannelMetadata>,
}
impl Discovered {
    pub fn find_uploads(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let playlist_ids = self
            .channels
            .keys()
            .map(|k| (k.clone(), k.playlist_long_form()))
            .collect::<Vec<_>>();
        let last_update = self.last_update;

        let handle = std::thread::spawn(move || {
            find_recent_uploads(playlist_ids, access_token, last_update, tx, context);
        });

        discovery.state = ChannelDiscoveryState::FindingUploads(FindingUploads {
            last_update: self.last_update,

            channels: std::mem::take(&mut self.channels),
            uploads: HashMap::new(),
            videos: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct FindingUploads {
    pub last_update: Timestamp,

    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,

    handle: JoinHandle<()>,
    incoming: Receiver<(ChannelId, Vec<(VideoId, VideoMetadata)>)>,
}

pub struct FoundUploads {
    pub last_update: Timestamp,

    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,
}
impl FoundUploads {
    pub fn filter(&mut self, discovery: &mut ChannelDiscovery) {
        discovery.state = ChannelDiscoveryState::FilteredByDate(FilteredByDate {
            new_videos: self
                .videos
                .iter()
                .filter(|(_, m)| m.published_at > self.last_update)
                .map(|(id, _)| id.clone())
                .collect(),

            channels: std::mem::take(&mut self.channels),
            videos: std::mem::take(&mut self.videos),

            uploads: std::mem::take(&mut self.uploads),
        })
    }
}

pub struct FilteredByDate {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,

    pub new_videos: HashSet<VideoId>,
}

impl FilteredByDate {
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
            videos: std::mem::take(&mut self.videos),

            uploads: std::mem::take(&mut self.uploads),
            new_videos: std::mem::take(&mut self.new_videos),

            playlist_items: HashSet::new(),
            total_playlist_items: None,

            incoming: rx,
            handle,
        });
    }
}
pub struct FindingPlaylistItems {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,

    pub new_videos: HashSet<VideoId>,

    pub playlist_items: HashSet<VideoId>,
    pub total_playlist_items: Option<i32>,

    handle: JoinHandle<()>,
    incoming: Receiver<(i32, Vec<VideoId>)>,
}

pub struct FoundPlaylistItems {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,

    pub new_videos: HashSet<VideoId>,
    pub playlist_items: HashSet<VideoId>,
}

impl FoundPlaylistItems {
    pub fn determine_video_languages(
        &mut self,
        discovery: &mut ChannelDiscovery,
        access_token: AccessToken,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();

        let videos = self
            .new_videos
            .difference(&self.playlist_items)
            .cloned()
            .collect::<Vec<_>>();

        let handle = std::thread::spawn(move || {
            determine_video_languages(videos, access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::DeterminingLanguages(DeterminingLanguages {
            channels: std::mem::take(&mut self.channels),
            videos: std::mem::take(&mut self.videos),

            uploads: std::mem::take(&mut self.uploads),
            new_videos: std::mem::take(&mut self.new_videos),
            playlist_items: std::mem::take(&mut self.playlist_items),

            video_languages: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct DeterminingLanguages {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,
    pub video_languages: HashMap<VideoId, String>,

    pub new_videos: HashSet<VideoId>,
    pub playlist_items: HashSet<VideoId>,

    handle: JoinHandle<()>,
    incoming: Receiver<(VideoId, String)>,
}

pub struct DeterminedLanguages {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,
    pub video_languages: HashMap<VideoId, String>,

    pub new_videos: HashSet<VideoId>,
    pub playlist_items: HashSet<VideoId>,
    pub excluded_languages: HashSet<String>,
}

impl DeterminedLanguages {
    pub fn add_to_playlist(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let playlist_id = discovery.playlist_id.clone();

        let exclude = self
            .playlist_items
            .iter()
            .chain(
                self.video_languages
                    .iter()
                    .filter(|(_, lang)| self.excluded_languages.contains(*lang))
                    .map(|(k, _)| k),
            )
            .cloned()
            .collect();

        let videos = self
            .new_videos
            .difference(&exclude)
            .cloned()
            .map(|id| {
                let meta = self.videos[&id].clone();
                (id, meta)
            })
            .collect::<Vec<_>>();

        let handle = std::thread::spawn(move || {
            let most_recent = add_to_playlist(videos, &playlist_id, &access_token, tx, context);

            set_playlist_metadata(playlist_id, &access_token, most_recent);
        });

        discovery.state = ChannelDiscoveryState::AddingToPlaylist(AddingToPlaylist {
            channels: std::mem::take(&mut self.channels),
            videos: std::mem::take(&mut self.videos),

            uploads: std::mem::take(&mut self.uploads),
            video_languages: std::mem::take(&mut self.video_languages),

            new_videos: std::mem::take(&mut self.new_videos),
            playlist_items: std::mem::take(&mut self.playlist_items),
            excluded_languages: std::mem::take(&mut self.excluded_languages),

            incoming: rx,
            handle,
        });
    }
}

pub struct AddingToPlaylist {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,
    pub video_languages: HashMap<VideoId, String>,

    pub new_videos: HashSet<VideoId>,
    pub playlist_items: HashSet<VideoId>,
    pub excluded_languages: HashSet<String>,

    handle: JoinHandle<()>,
    incoming: Receiver<VideoId>,
}

pub struct Done {
    pub channels: HashMap<ChannelId, ChannelMetadata>,
    pub videos: HashMap<VideoId, VideoMetadata>,

    pub uploads: HashMap<ChannelId, Vec<VideoId>>,
    pub video_languages: HashMap<VideoId, String>,

    pub new_videos: HashSet<VideoId>,
    pub playlist_items: HashSet<VideoId>,
}

pub enum ChannelDiscoveryState {
    Idle(Idle),

    Discovering(Discovering),
    Discovered(Discovered),

    FindingUploads(FindingUploads),
    FoundUploads(FoundUploads),

    FilteredByDate(FilteredByDate),

    FindingPlaylistItems(FindingPlaylistItems),
    FoundPlaylistItems(FoundPlaylistItems),

    DeterminingLanguages(DeterminingLanguages),
    DeterminedLanguages(DeterminedLanguages),

    AddingToPlaylist(AddingToPlaylist),
    Done(Done),
}

impl Default for ChannelDiscoveryState {
    fn default() -> Self {
        ChannelDiscoveryState::Idle(Idle {})
    }
}

// TODO: etag and caching
fn get_all_channels(
    token: &AccessToken,
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
    channel: Sender<(ChannelId, Vec<(VideoId, VideoMetadata)>)>,
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

                    (video_id,
                    VideoMetadata {
                        title,
                        thumbnail: thumbnail.url.unwrap(),
                        published_at,
                        position,
                    })
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

fn determine_video_languages(
    video_ids: Vec<VideoId>,
    token: AccessToken,
    channel: Sender<(VideoId, String)>,
    context: Context,
) {
    for chunk in video_ids.chunks(50) {
        let url = "https://www.googleapis.com/youtube/v3/videos?part=id,snippet&maxResults=50";
        let url = format!(
            "{url}&id={}",
            chunk
                .iter()
                .map(VideoId::as_ref)
                .collect::<Vec<_>>()
                .join(",")
        );

        let json = ureq::get(url.as_ref())
            .set("Authorization", &format!("Bearer {}", token.secret()))
            .call()
            .unwrap()
            .into_json::<VideoListResponse>()
            .unwrap();

        let items = json.items.unwrap();

        for item in items {
            channel
                .send((
                    VideoId::new(item.id.unwrap()),
                    item.snippet
                        .unwrap()
                        .default_audio_language
                        .unwrap_or_else(|| String::from("unknown")),
                ))
                .unwrap();
        }
        tracing::info!("chunk");
        context.request_repaint();
    }
}

fn add_to_playlist(
    videos: Vec<(VideoId, VideoMetadata)>,
    playlist_id: &PlaylistId,
    access_token: &AccessToken,
    channel: Sender<VideoId>,
    context: Context,
) -> Timestamp {
    assert!(!videos.is_empty(), "if videos is empty, this breaks");

    let mut most_recent_upload = Timestamp::MIN;

    let url = format!(
        "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet&playlistId={playlist_id}"
    );

    for (video_id, meta) in videos {
        ureq::post(url.as_ref())
            .set(
                "Authorization",
                &format!("Bearer {}", access_token.secret()),
            )
            .send_json(PlaylistItem {
                snippet: Some(PlaylistItemSnippet {
                    playlist_id: Some(playlist_id.to_string()),
                    resource_id: Some(ResourceId {
                        // TODO: resource id should be an enum with a tag
                        kind: Some(String::from("youtube#video")),
                        video_id: Some(video_id.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap();

        channel.send(video_id).unwrap();
        most_recent_upload = meta.published_at.max(most_recent_upload);

        tracing::info!("video");
        context.request_repaint();
    }

    most_recent_upload
}

fn get_playlist(playlist_id: &PlaylistId, token: &AccessToken) -> Playlist {
    let url =
        format!("https://www.googleapis.com/youtube/v3/playlists?part=snippet,id&id={playlist_id}");

    let json = ureq::get(url.as_ref())
        .set("Authorization", &format!("Bearer {}", token.secret()))
        .call()
        .unwrap()
        .into_json::<PlaylistListResponse>()
        .unwrap();

    json.items.unwrap().pop().unwrap()
}

fn get_playlist_metadata(playlist_id: PlaylistId, token: &AccessToken) -> Timestamp {
    let playlist = get_playlist(&playlist_id, token);
    let description = playlist.snippet.unwrap().description.unwrap();

    let pattern = " | Last update: ";
    let timestamp_pos = description.find(pattern).unwrap() + pattern.len();

    description[timestamp_pos..]
        .trim()
        .parse::<Timestamp>()
        .unwrap()
}

fn set_playlist_metadata(playlist_id: PlaylistId, token: &AccessToken, last_update: Timestamp) {
    let playlist = get_playlist(&playlist_id, token);

    let snippet = playlist.snippet.unwrap();
    let description = snippet.description.unwrap();

    let pattern = " | Last update: ";
    let timestamp_pos = description.find(pattern).unwrap() + pattern.len();

    let description = format!("{}{}", &description[..timestamp_pos], last_update);

    let url =
        format!("https://www.googleapis.com/youtube/v3/playlists?part=snippet&id={playlist_id}");

    ureq::put(url.as_ref())
        .set("Authorization", &format!("Bearer {}", token.secret()))
        .send_json(Playlist {
            id: playlist.id,
            snippet: Some(PlaylistSnippet {
                description: Some(description),
                ..snippet
            }),
            ..Default::default()
        })
        .unwrap();
}
