use std::{sync::Arc, time::Duration};

use entity_types::subscription_queue::SubscriptionAction;
use jiff::{SignedDuration, Timestamp};
use sea_orm::{DatabaseConnection, DbErr};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::database::{ActiveSubscriptions, SubscriptionQueue};

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

    tracing::info!("shutting down");

    Ok(())
}
