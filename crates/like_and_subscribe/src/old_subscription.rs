use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, Mutex},
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
use tracing::{Instrument, debug, debug_span, error, info, trace, warn};

#[derive(Debug, Default, Serialize, Clone)]
pub struct YoutubeChannelSubscription {
    pub name: String,
    pub subscription_expiration: Option<Zoned>,
    pub stale: bool,
}

pub async fn youtube_subscription_manager(
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    hostname: String,
    client: reqwest::Client,
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    subscriptions: Arc<Mutex<HashMap<String, YoutubeChannelSubscription>>>,
) {
    let mut last_etag: Option<String> = None;

    let callback = &format!("https://{hostname}/pubsub");

    let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60)); // One hour
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // FIXME: somehow this loop gets stuck. This is likely due to a key expiring and it spinning up a server to do OAUTH.... I need to fix that
    loop {
        tokio::select! {
            _ = ticker.tick() => {},
            _ = shutdown.recv() => {
                tracing::info!("subscription manager shutting down");
                return;
            }
        }

        async {
            let token = youtube
                .auth
                .get_token(&[Scope::Readonly.as_ref()])
                .await
                .map_err(|e| eyre!("{e}"))
                .wrap_err("unable to get authentication token")
                .expect("token should be refreshed")
                .expect("token should exist"); // TODO: FIXME: remove unwrap

            // Mark all existing subscriptions stale
            subscriptions
                .lock()
                .expect("mutex should not be poisoned")
                .values_mut()
                .for_each(|s| s.stale = true);

            get_all_subscriptions(&client, &subscriptions, &mut last_etag, token).await;

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
                                // TODO: break out into function and merge with test
                                let now = Zoned::now();

                                // re-subscribe if expring in a day
                                expiration.duration_since(&now)
                                    <= Span::new().days(1).to_duration(&now).unwrap()
                            }
                            None => true,
                        })
                        .map(|(a, b)| (Mode::Subscribe, (a.clone(), b.clone()))),
                );

                // stream::iter(action_queue).for_each_concurrent(10, |(mode, (channel_id, YoutubeChannelSubscription { name, .. }))| {
                //     let client = client.clone();

                //     let span = debug_span!("subscription_update", channel_id, name, ?mode);

                //     // TODO: make this a function?
                //     async move {
                //         let request = client
                //             .post("https://pubsubhubbub.appspot.com/subscribe")
                //             .form(&HubRequest {
                //                 mode,
                //                 callback,
                //                 verify: Verify::Synchronous,
                //                 topic: format!(
                //                     "https://www.youtube.com/xml/feeds/videos.xml?channel_id={channel_id}"
                //                 ),
                //             })
                //             .build()
                //             .expect("request should be well formed");

                //         let response = match client.execute(request).await {
                //             Ok(response) => response,
                //             Err(error) => {
                //                 // TODO: implement retries? put back on the queue?
                //                 // TODO: keep track of subscribed channels?? how do we know whats new?
                //                 warn!(%error, "failed to subscribe to a youtube channel");
                //                 return;
                //             }
                //         };

                //         if response.status() == StatusCode::TOO_MANY_REQUESTS {
                //             // TODO: retries from too many requests
                //             error!("too many requests");
                //             return;
                //         }

                //         if !response.status().is_success() {
                //             let status_code = response.status().as_u16();
                //             warn!(status_code, "server returned error");
                //             return;
                //         }

                //         trace!("updated subscription")
                //     }
                //     .instrument(span)
                // }).await;
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
        }
        .instrument(debug_span!("subscription_manage"))
        .await
    }
}
