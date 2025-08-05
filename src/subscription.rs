use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Mutex,
    time::Duration,
};

use axum::http::{HeaderMap, HeaderValue};
use color_eyre::eyre::{Context as _, eyre};
use futures::{StreamExt, stream};
use google_youtube3::{
    YouTube,
    api::{Scope, SubscriptionListResponse},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use jiff::{Span, Zoned};
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use tracing::{Instrument, debug, error, info, trace, trace_span, warn};

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Serialize)]
pub struct HubRequest<'s> {
    #[serde(rename = "hub.topic")]
    pub(crate) topic: String,
    #[serde(rename = "hub.callback")]
    pub(crate) callback: &'s str,
    #[serde(rename = "hub.mode")]
    pub(crate) mode: Mode,
    #[serde(rename = "hub.verify")]
    pub(crate) verify: Verify,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub enum Verify {
    #[serde(rename = "async")]
    Asynchronous,
    #[serde(rename = "sync")]
    Synchronous,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct YoutubeChannelSubscription {
    pub name: String,
    pub subscription_expiration: Option<Zoned>,
    pub stale: bool,
}

pub async fn youtube_subscription_manager(
    hostname: String,
    client: &reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    subscriptions: &Mutex<HashMap<String, YoutubeChannelSubscription>>,
) -> color_eyre::Result<()> {
    let mut last_etag: Option<String> = None;

    let callback = &format!("https://{hostname}/pubsub");

    let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60)); // One hour
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        async {
            let token = youtube
                .auth
                .get_token(&[Scope::Readonly.as_ref()])
                .await
                .map_err(|e| eyre!("{e}"))
                .wrap_err("unable to get authentication token").unwrap()
                .unwrap(); // TODO: FIXME: remove unwrap

            // Mark all existing subscriptions stale
            subscriptions
                .lock()
                .unwrap()
                .values_mut()
                .for_each(|s| s.stale = true);

            get_all_subscriptions(client, subscriptions, &mut last_etag, token).await;

            // Prune stale entries
            {
                let mut action_queue = subscriptions
                    .lock()
                    .unwrap()
                    .extract_if(|_, sub| sub.stale)
                    .inspect(|(channel_id, sub)| {
                        debug!(?channel_id, name = sub.name, "removing stale subscription");
                    })
                    .map(|x| (Mode::Unsubscribe, x))
                    .collect::<Vec<_>>();

                action_queue.extend(
                    subscriptions
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(_, s)| match s.subscription_expiration.as_ref() {
                            Some(expiration) => {
                                let now = Zoned::now();

                                // re-subscribe if expring in a day
                                expiration.duration_since(&now)
                                    <= Span::new().days(1).to_duration(&now).unwrap()
                            }
                            None => true,
                        })
                        .map(|(a, b)| (Mode::Subscribe, (a.clone(), b.clone()))),
                );

                stream::iter(action_queue).for_each_concurrent(10, |(mode, (channel_id, YoutubeChannelSubscription { name, .. }))| {
                        let client = client.clone();

                        let span = trace_span!("subscription_update", channel_id, name, ?mode);

                        // TODO: make this a function?
                        async move {
                            let request = client
                                .post("https://pubsubhubbub.appspot.com/subscribe")
                                .form(&HubRequest {
                                    mode,
                                    callback,
                                    verify: Verify::Synchronous,
                                    topic: format!(
                                        "https://www.youtube.com/xml/feeds/videos.xml?channel_id={channel_id}"
                                    ),
                                })
                                .build()
                                .expect("request should be well formed");

                            let response = match client.execute(request).await {
                                Ok(response) => response,
                                Err(error) => {
                                    // TODO: implement retries? put back on the queue?
                                    // TODO: keep track of subscribed channels?? how do we know whats new?
                                    warn!(%error, "failed to subscribe to a youtube channel");
                                    return;
                                }
                            };

                            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                                // TODO: retries from too many requests
                                error!("too many requests");
                                return;
                            }

                            if !response.status().is_success() {
                                let status_code = response.status().as_u16();
                                warn!(status_code, "server returned error");
                                return;
                            }

                            trace!("end")
                        }
                        .instrument(span)
                    }).await;
            }

            let subscriptions = subscriptions.lock().unwrap();
            let total_count = subscriptions.len();
            let stale_count = subscriptions.values().filter(|s| s.stale).count();
            let subscribed_count = subscriptions
                .values()
                .filter(|s| s.subscription_expiration.is_some())
                .count();
            let soonest_expiration = subscriptions
                .values()
                .flat_map(|s| s.subscription_expiration.as_ref())
                .max()
                .map(|exp| exp.to_string());

            info!(
                total_count,
                stale_count, subscribed_count, soonest_expiration, "subscription update end"
            );
        }.instrument(trace_span!("subscription_manage")).await
    }
}

async fn get_all_subscriptions(
    client: &reqwest::Client,
    subscriptions: &Mutex<HashMap<String, YoutubeChannelSubscription>>,
    last_etag: &mut Option<String>,
    token: String,
) {
    let mut page_token = None;
    let url = "https://www.googleapis.com/youtube/v3/subscriptions?part=snippet,contentDetails&mine=true&maxResults=50";

    // Pagination handling
    loop {
        let url = if let Some(page_token) = &page_token {
            format!("{url}&pageToken={page_token}")
        } else {
            url.to_string()
        };

        let headers = if let Some(etag) = last_etag {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(etag).unwrap());
            headers
        } else {
            HeaderMap::new()
        };

        let response = client
            .get(url)
            .bearer_auth(&token)
            .headers(headers)
            .send()
            .await
            .unwrap();

        let status = response.status();

        if status == StatusCode::NOT_MODIFIED {
            info!("not changed");

            // Mark all existing subscriptions as not stale
            subscriptions
                .lock()
                .unwrap()
                .values_mut()
                .for_each(|s| s.stale = false);

            break;
        }

        if !status.is_success() {
            warn!(status=%status, status_message=status.canonical_reason(), "failed to paginate all subscriptions");
            break;
        }

        let json = response.json::<SubscriptionListResponse>().await.unwrap();

        info!(json.etag, json.next_page_token);

        if page_token.is_none() {
            *last_etag = json.etag;
        }

        let items = json.items.unwrap();

        let mut subscriptions = subscriptions.lock().unwrap();
        for subscription in items {
            let snippet = subscription.snippet.unwrap();
            let resource = snippet.resource_id.unwrap();

            assert_eq!(resource.kind.as_deref(), Some("youtube#channel"));

            let channel_id = resource.channel_id.unwrap();
            let channel_name = snippet.title.unwrap();

            // Either add item or mark as fresh
            match subscriptions.entry(channel_id.clone()) {
                Entry::Occupied(mut occupied_entry) => {
                    occupied_entry.get_mut().stale = false;
                }
                Entry::Vacant(vacant_entry) => {
                    vacant_entry.insert(YoutubeChannelSubscription {
                        name: channel_name,
                        subscription_expiration: None,
                        stale: false,
                    });
                }
            }
        }

        page_token = json.next_page_token;

        if page_token.is_none() {
            break;
        }
    }
}
