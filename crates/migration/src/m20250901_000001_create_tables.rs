use sea_orm_migration::{prelude::*, schema};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(OAuth::create()).await?;
        manager.create_table(KnownChannels::create()).await?;
        manager.create_table(KnownVideos::create()).await?;
        manager.create_table(ActiveSubscriptions::create()).await?;
        manager.create_table(SubscriptionQueue::create()).await?;
        manager
            .create_table(SubscriptionQueueResult::create())
            .await?;
        manager.create_table(VideoQueue::create()).await?;
        manager.create_table(VideoQueueResult::create()).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OAuth::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(KnownChannels::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(KnownVideos::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ActiveSubscriptions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SubscriptionQueue::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(SubscriptionQueueResult::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(VideoQueue::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(VideoQueueResult::Table).to_owned())
            .await?;

        Ok(())
    }
}

trait TableTrait {
    fn create() -> TableCreateStatement;
}

#[derive(DeriveIden)]
enum OAuth {
    Table,
    RowId,

    AccessToken,
    RefreshToken,
    ExpiresAt,
}

impl TableTrait for OAuth {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(OAuth::Table)
            .if_not_exists()
            .col(schema::pk_auto(OAuth::RowId))
            .col(schema::text(OAuth::AccessToken))
            .col(schema::text(OAuth::RefreshToken))
            .col(schema::big_integer(OAuth::ExpiresAt))
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum KnownChannels {
    Table,
    ChannelId,

    ChannelName,
    ChannelProfilePicture,
}

impl TableTrait for KnownChannels {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(KnownChannels::Table)
            .if_not_exists()
            .col(schema::text(KnownChannels::ChannelId).primary_key())
            .col(schema::text(KnownChannels::ChannelName))
            .col(schema::text(KnownChannels::ChannelProfilePicture))
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum KnownVideos {
    Table,
    VideoId,

    ChannelId,
}

impl TableTrait for KnownVideos {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(KnownVideos::Table)
            .if_not_exists()
            .col(schema::text(KnownVideos::VideoId).primary_key())
            .col(schema::text(KnownVideos::ChannelId))
            .foreign_key(
                ForeignKey::create()
                    .name("fk-known_videos-channel_id")
                    .from(KnownVideos::Table, KnownVideos::ChannelId)
                    .to(KnownChannels::Table, KnownChannels::ChannelId),
            )
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum ActiveSubscriptions {
    Table,
    ChannelId,

    Expiration,
}

impl TableTrait for ActiveSubscriptions {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(ActiveSubscriptions::Table)
            .if_not_exists()
            .col(schema::text(ActiveSubscriptions::ChannelId).primary_key())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-active_subscriptions-channel_id")
                    .from(ActiveSubscriptions::Table, ActiveSubscriptions::ChannelId)
                    .to(KnownChannels::Table, KnownChannels::ChannelId),
            )
            .col(schema::big_integer(ActiveSubscriptions::Expiration))
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum SubscriptionQueue {
    Table,
    Id,

    ChannelId,
    Action,
    Timestamp,
}

impl TableTrait for SubscriptionQueue {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(SubscriptionQueue::Table)
            .if_not_exists()
            .col(schema::pk_auto(SubscriptionQueue::Id))
            .col(schema::text(SubscriptionQueue::ChannelId))
            .foreign_key(
                ForeignKey::create()
                    .name("fk-subscription_queue-channel_id")
                    .from(SubscriptionQueue::Table, SubscriptionQueue::ChannelId)
                    .to(KnownChannels::Table, KnownChannels::ChannelId),
            )
            .col(schema::text(SubscriptionQueue::Action))
            .col(schema::big_integer(SubscriptionQueue::Timestamp))
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum SubscriptionQueueResult {
    Table,
    QueueId,

    Error,
    Timestamp,
}

impl TableTrait for SubscriptionQueueResult {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(SubscriptionQueueResult::Table)
            .if_not_exists()
            .col(schema::integer(SubscriptionQueueResult::QueueId).primary_key())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-subscription_queue_result-queue_id")
                    .from(
                        SubscriptionQueueResult::Table,
                        SubscriptionQueueResult::QueueId,
                    )
                    .to(SubscriptionQueue::Table, SubscriptionQueue::Id),
            )
            .col(schema::text_null(SubscriptionQueueResult::Error))
            .col(schema::big_integer(SubscriptionQueueResult::Timestamp))
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum VideoQueue {
    Table,
    Id,

    ChannelId,
    VideoId,
    Title,
    PublishedAt,
    UpdatedAt,
    Timestamp,
}

impl TableTrait for VideoQueue {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(VideoQueue::Table)
            .if_not_exists()
            .col(schema::pk_auto(VideoQueue::Id))
            .col(schema::text(VideoQueue::ChannelId))
            .foreign_key(
                ForeignKey::create()
                    .name("fk-video_queue-channel_id")
                    .from(VideoQueue::Table, VideoQueue::ChannelId)
                    .to(KnownChannels::Table, KnownChannels::ChannelId),
            )
            .col(schema::text(VideoQueue::VideoId))
            .foreign_key(
                ForeignKey::create()
                    .name("fk-video_queue-video_id")
                    .from(VideoQueue::Table, VideoQueue::VideoId)
                    .to(KnownVideos::Table, KnownVideos::VideoId),
            )
            .col(schema::text(VideoQueue::Title))
            .col(schema::big_integer(VideoQueue::PublishedAt))
            .col(schema::big_integer(VideoQueue::UpdatedAt))
            .col(schema::big_integer(VideoQueue::Timestamp))
            .to_owned()
    }
}

#[derive(DeriveIden)]
enum VideoQueueResult {
    Table,
    QueueId,

    Action,
    ShortsRedirect,
    Visibility,
    Duration,
    Timestamp,
}

impl TableTrait for VideoQueueResult {
    fn create() -> TableCreateStatement {
        Table::create()
            .table(VideoQueueResult::Table)
            .if_not_exists()
            .col(schema::integer(VideoQueueResult::QueueId).primary_key())
            .foreign_key(
                ForeignKey::create()
                    .name("fk-video_queue_result-queue_id")
                    .from(VideoQueueResult::Table, VideoQueueResult::QueueId)
                    .to(VideoQueue::Table, VideoQueue::Id),
            )
            .col(schema::text(VideoQueueResult::Action))
            .col(schema::boolean(VideoQueueResult::ShortsRedirect))
            .col(schema::text(VideoQueueResult::Visibility))
            .col(schema::big_integer(VideoQueueResult::Duration))
            .col(schema::big_integer(VideoQueueResult::Timestamp))
            .to_owned()
    }
}
