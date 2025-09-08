use entity::{active_subscriptions, video_queue};
use jiff::Timestamp;
use jiff_sea_orm_compat::JiffTimestampMilliseconds;
use sea_orm::{ActiveValue, DatabaseConnection, DbErr, EntityTrait as _, IntoActiveModel as _};

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
        .exec(db)
        .await?;

        Ok(())
    }
}
