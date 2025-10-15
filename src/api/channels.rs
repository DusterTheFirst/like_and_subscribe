use std::{
    borrow::Cow,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use eframe::egui::{
    Context,
    ahash::{HashMap, HashMapExt},
};
use google_youtube3::api::SubscriptionListResponse;
use oauth2::{AccessToken, ureq};

pub struct ChannelDiscovery {
    discovered_channels: HashMap<String, ChannelMetadata>,

    context: Context,

    job: Option<Job>,
}

struct Job {
    incoming_channels: Receiver<(i32, String, ChannelMetadata)>,
    expected_channels: Option<i32>,

    handle: JoinHandle<()>,
}

impl ChannelDiscovery {
    pub fn new(context: Context) -> Self {
        Self {
            discovered_channels: HashMap::new(),
            context,
            job: None,
        }
    }

    pub fn state(&'_ mut self) -> ChannelDiscoveryState<'_> {
        match &mut self.job {
            None => ChannelDiscoveryState::Idle(Idle { parent: self }),
            Some(job) => {
                while let Ok((expected, channel_id, metadata)) = job.incoming_channels.try_recv() {
                    self.discovered_channels.insert(channel_id, metadata);
                    job.expected_channels = Some(expected);
                }

                if job.handle.is_finished() {
                    ChannelDiscoveryState::Complete {
                        discovered_channels: &self.discovered_channels,
                    }
                } else {
                    ChannelDiscoveryState::Discovering {
                        discovered_channels: &self.discovered_channels,
                        total_channel_count: job.expected_channels,
                    }
                }
            }
        }
    }
}

pub struct Idle<'a> {
    parent: &'a mut ChannelDiscovery,
}
impl<'a> Idle<'a> {
    pub fn start(&mut self, access_token: AccessToken) {
        let (tx, rx) = std::sync::mpsc::channel();
        let context = self.parent.context.clone();

        let handle = std::thread::spawn(|| {
            get_all_channels(access_token, tx, context);
        });

        self.parent.job = Some(Job {
            incoming_channels: rx,
            expected_channels: None,
            handle,
        });
    }
}

pub enum ChannelDiscoveryState<'a> {
    Idle(Idle<'a>),
    Discovering {
        discovered_channels: &'a HashMap<String, ChannelMetadata>,
        total_channel_count: Option<i32>,
    },
    Complete {
        discovered_channels: &'a HashMap<String, ChannelMetadata>,
    },
}

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
