use std::{
    borrow::Cow,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use eframe::egui::{
    Context,
    ahash::{HashMap, HashMapExt},
};
use google_youtube3::api::{ChannelListResponse, SubscriptionListResponse};
use oauth2::{AccessToken, ureq};

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
                mut discovered_channels,
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
                        discovered_channels,
                    })
                } else {
                    ChannelDiscoveryState::Discovering(Discovering {
                        discovered_channels,
                        total_channel_count,
                        handle,
                        incoming,
                    })
                }
            }
            ChannelDiscoveryState::Elaborating(Elaborating {
                discovered_channels,
                mut elaboration,
                handle,
                incoming,
            }) => {
                while let Ok((expected, metadata)) = incoming.try_recv() {
                    elaboration.insert(expected, metadata);
                }

                if handle.is_finished() {
                    ChannelDiscoveryState::Elaborated(Elaborated {
                        discovered_channels,
                        elaboration,
                    })
                } else {
                    ChannelDiscoveryState::Elaborating(Elaborating {
                        discovered_channels,
                        elaboration,

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
            discovered_channels: HashMap::new(),
            total_channel_count: None,

            incoming: rx,
            handle,
        });
    }
}

pub struct Discovering {
    pub discovered_channels: HashMap<String, ChannelMetadata>,
    pub total_channel_count: Option<i32>,

    handle: JoinHandle<()>,
    incoming: Receiver<(i32, String, ChannelMetadata)>,
}

pub struct Discovered {
    pub discovered_channels: HashMap<String, ChannelMetadata>,
}
impl Discovered {
    pub fn elaborate(&mut self, discovery: &mut ChannelDiscovery, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = discovery.context.clone();
        let channel_ids = self.discovered_channels.keys().cloned().collect::<Vec<_>>();

        let handle = std::thread::spawn(|| {
            elaborate_all_channels(channel_ids, access_token, tx, context);
        });

        discovery.state = ChannelDiscoveryState::Elaborating(Elaborating {
            discovered_channels: std::mem::take(&mut self.discovered_channels),
            elaboration: HashMap::new(),

            incoming: rx,
            handle,
        });
    }
}

pub struct Elaborating {
    pub discovered_channels: HashMap<String, ChannelMetadata>,
    pub elaboration: HashMap<String, String>,

    handle: JoinHandle<()>,
    incoming: Receiver<(String, String)>,
}

pub struct Elaborated {
    pub discovered_channels: HashMap<String, ChannelMetadata>,
    pub elaboration: HashMap<String, String>,
}

pub enum ChannelDiscoveryState {
    Idle(Idle),
    Discovering(Discovering),
    Discovered(Discovered),
    Elaborating(Elaborating),
    Elaborated(Elaborated),
}

impl Default for ChannelDiscoveryState {
    fn default() -> Self {
        ChannelDiscoveryState::Idle(Idle {})
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct ChannelMetadata {
    pub name: String,
    pub profile_picture: String,
}

// TODO: etag and caching
fn get_all_channels(
    token: AccessToken,
    channel: Sender<(i32, String, ChannelMetadata)>,
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
    channel_ids: Vec<String>,
    token: AccessToken,
    channel: Sender<(String, String)>,
    context: Context,
) {
    for chunk in channel_ids.chunks(50) {
        let url =
            "https://www.googleapis.com/youtube/v3/channels?part=id,contentDetails&maxResults=50";
        let url = format!("{url}&id={}", chunk.join(","));

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
                    item.id.unwrap(),
                    item.content_details
                        .unwrap()
                        .related_playlists
                        .unwrap()
                        .uploads
                        .unwrap(),
                ))
                .unwrap();
        }
        tracing::info!("chunk");
        context.request_repaint();
    }
}
