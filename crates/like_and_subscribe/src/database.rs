use std::error::Error;

use entity::{
    SubscriptionQueueToActiveSubscriptions, active_subscriptions, subscription_queue,
    subscription_queue_result, video_queue,
};
use entity_types::{
    jiff_compat::JiffTimestampMilliseconds, subscription_queue::SubscriptionAction,
};
use futures::{Stream, TryStreamExt};
use jiff::{SignedDuration, Timestamp};
use migration::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait as _, DatabaseConnection, DbErr, EntityTrait as _,
    IntoActiveModel as _, Iterable, QueryFilter, QuerySelect,
};
use tokio::sync::Notify;

use crate::feed;

pub struct VideoQueue;

impl VideoQueue {
    pub async fn new_video(db: &DatabaseConnection, entry: feed::Entry) -> Result<(), DbErr> {
        video_queue::Entity::insert(video_queue::ActiveModel {
            id: ActiveValue::NotSet,
            channel_id: ActiveValue::Set(entry.channel_id),
            video_id: ActiveValue::Set(entry.video_id),

            title: ActiveValue::Set(entry.title),

            published_at: ActiveValue::Set(JiffTimestampMilliseconds(entry.published)),
            updated_at: ActiveValue::Set(JiffTimestampMilliseconds(entry.updated)),

            timestamp: ActiveValue::Set(JiffTimestampMilliseconds(Timestamp::now())),
        })
        .exec(db)
        .await?;

        Ok(())
    }
}

pub struct ActiveSubscriptions;

impl ActiveSubscriptions {
    pub async fn remove_subscription(db: &DatabaseConnection, id: String) -> Result<(), DbErr> {
        active_subscriptions::Entity::delete_by_id(id)
            .exec(db)
            .await?;

        Ok(())
    }

    pub async fn add_subscription(
        db: &DatabaseConnection,
        channel_id: String,
        expiration: Timestamp,
    ) -> Result<(), DbErr> {
        active_subscriptions::Entity::insert(
            active_subscriptions::Model {
                channel_id: channel_id.to_owned(),
                expiration: JiffTimestampMilliseconds(expiration),
            }
            .into_active_model(),
        )
        .on_conflict(
            OnConflict::column(active_subscriptions::Column::ChannelId)
                .update_columns(active_subscriptions::Column::iter())
                .to_owned(),
        )
        .exec(db)
        .await?;

        Ok(())
    }

    pub async fn get_soonest_expiration(
        db: &DatabaseConnection,
    ) -> Result<Option<Timestamp>, DbErr> {
        Ok(active_subscriptions::Entity::find()
            .select_only()
            .column_as(active_subscriptions::Column::Expiration.min(), "expiration")
            .into_tuple::<JiffTimestampMilliseconds>()
            .one(db)
            .await?
            .map(|j| j.0))
    }

    pub async fn get_expiring(
        db: &DatabaseConnection,
        expires_before: Timestamp,
    ) -> Result<Vec<active_subscriptions::Model>, DbErr> {
        active_subscriptions::Entity::find()
            .filter(
                active_subscriptions::Column::Expiration
                    .lt(JiffTimestampMilliseconds(expires_before)),
            )
            .all(db)
            .await
    }
}

pub struct SubscriptionQueue;

impl SubscriptionQueue {
    pub async fn add_actions(
        db: &DatabaseConnection,
        notify: &Notify,
        actions: impl IntoIterator<Item = (String, SubscriptionAction)>, // TODO: newtype channel id and other ids
    ) -> Result<(), DbErr> {
        subscription_queue::Entity::insert_many(actions.into_iter().map(|(channel_id, action)| {
            subscription_queue::ActiveModel {
                id: ActiveValue::NotSet,
                channel_id: ActiveValue::Set(channel_id),
                action: ActiveValue::Set(action),
                timestamp: ActiveValue::Set(JiffTimestampMilliseconds(Timestamp::now())),
            }
        }))
        .exec(db)
        .await?;

        notify.notify_one();

        Ok(())
    }

    pub async fn get_pending_actions<'db>(
        db: &'db DatabaseConnection,
    ) -> Result<impl Stream<Item = Result<SubscriptionQueueItem, DbErr>> + Send + 'db, DbErr> {
        Ok(subscription_queue::Entity::find()
            .left_join(subscription_queue_result::Entity)
            .filter(subscription_queue_result::Column::Timestamp.is_null())
            .find_also_linked(SubscriptionQueueToActiveSubscriptions)
            .stream(db)
            .await?
            .map_ok(|(queue_item, active_subscription)| SubscriptionQueueItem {
                queue_item,
                active_subscription,
                db: db.clone(),
            }))
    }
}

pub struct SubscriptionQueueItem {
    queue_item: subscription_queue::Model,
    active_subscription: Option<active_subscriptions::Model>,
    db: DatabaseConnection,
}

impl SubscriptionQueueItem {
    pub async fn process<F, E>(self, function: F) -> Result<(), DbErr>
    where
        F: AsyncFnOnce(
                &subscription_queue::Model,
                Option<&active_subscriptions::Model>,
            ) -> Result<(), E>
            + Send
            + Sync,
        E: Error + Send + Sync,
    {
        let result = function(&self.queue_item, self.active_subscription.as_ref()).await;

        let model = match result {
            Ok(()) => subscription_queue_result::Model {
                queue_id: self.queue_item.id,
                error: None,
                timestamp: JiffTimestampMilliseconds(Timestamp::now()),
            },
            Err(error) => {
                // TODO: how to handle retries? do we just wait for the subscription manager?
                tracing::error!(%error, "failed to process subscription queue item");

                subscription_queue_result::Model {
                    queue_id: self.queue_item.id,
                    error: Some(error.to_string()),
                    timestamp: JiffTimestampMilliseconds(Timestamp::now()),
                }
            }
        };

        subscription_queue_result::Entity::insert(model.into_active_model())
            .exec(&self.db)
            .await?;

        Ok(())
    }
}
