use std::{sync::Arc, time::Duration};

use entity_types::subscription_queue::SubscriptionAction;
use futures::TryStreamExt;
use jiff::{SignedDuration, Timestamp};
use reqwest::Client;
use sea_orm::{DatabaseConnection, DbErr};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::database::{ActiveSubscriptions, SubscriptionQueue};

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

fn topic(channel_id: &str) -> String {
    format!("https://www.youtube.com/xml/feeds/videos.xml?channel_id={channel_id}")
}

pub async fn pubsub_refresh(
    shutdown: CancellationToken,
    database: DatabaseConnection,
    notify: Arc<Notify>,
) -> Result<(), DbErr> {
    let refresh_window = SignedDuration::from_secs(60 * 60 * 24);
    let refresh_delay = SignedDuration::from_secs(60 * 60);

    loop {
        let soonest_expiration = ActiveSubscriptions::get_soonest_expiration(&database)
            .await
            .inspect_err(|error| tracing::error!(%error, "failed to get soonest expiration"))?;

        let delay = match soonest_expiration {
            Some(expiration) => Timestamp::now()
                .duration_until(expiration)
                .saturating_sub(refresh_window.saturating_sub(refresh_delay))
                .try_into()
                .expect("duration should never be negative"),

            None => Duration::from_secs(24 * 60 * 60), // No subscriptions, wait a day
        };

        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(delay) => {},
        }

        let expiring =
            ActiveSubscriptions::get_expiring(&database, Timestamp::now() + refresh_window)
                .await
                .inspect_err(
                    |error| tracing::error!(%error, "failed to get expiring subscriptions"),
                )?;

        SubscriptionQueue::add_actions(
            &database,
            &notify,
            expiring
                .into_iter()
                .map(|model| (model.channel_id, SubscriptionAction::Refresh)),
        )
        .await
        .inspect_err(|error| tracing::error!(%error, "failed to insert subscription refreshes"))?
    }

    Ok(())
}

pub async fn pubsub_queue_consumer(
    shutdown: CancellationToken,
    database: DatabaseConnection,
    notify: Arc<Notify>,
    client: Client,
    callback: String,
) -> Result<(), DbErr> {
    loop {
        let actions = SubscriptionQueue::get_pending_actions(&database)
            .await
            .inspect_err(
                |error| tracing::error!(%error, "failed to get pending actions from database"),
            )?;

        actions
            .try_for_each_concurrent(10, async |queue_item| {
                queue_item
                    .process::<_, reqwest::Error>(async |queue_item, active_subscription| {
                        let topic = topic(&queue_item.channel_id);

                        let mode = match queue_item.action {
                            SubscriptionAction::Subscribe => Mode::Subscribe,
                            SubscriptionAction::Unsubscribe => Mode::Unsubscribe,
                            SubscriptionAction::Refresh if active_subscription.is_some() => {
                                Mode::Subscribe
                            }
                            SubscriptionAction::Refresh => {
                                tracing::warn!(
                                    ?queue_item,
                                    "refresh action queued without an active subscription"
                                );
                                return Ok(());
                            }
                        };

                        let request = client
                            .post("https://pubsubhubbub.appspot.com/subscribe")
                            .form(&HubRequest {
                                mode,
                                callback: &callback,
                                verify: Verify::Synchronous,
                                topic,
                            })
                            .build()?;

                        client.execute(request).await?.error_for_status()?;

                        Ok(())
                    })
                    .await
            })
            .await
            .inspect_err(
                |error| tracing::error!(%error, "failed to get pending action from database"),
            )?;

        tokio::select! {
            _ = notify.notified() => {},
            _ = shutdown.cancelled() => break,
        }
    }

    tracing::info!("shutting down");

    Ok(())
}
