use askama::Template;
use axum::{extract::State, response::Html};
use axum_extra::response::InternalServerError;
use entity::video_queue_result;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait as _};

use crate::database::{self, OAuth};

#[derive(Template)]
#[template(path = "dashboard.html")]
struct Dashboard {
    oauth_token: Option<database::Authentication>,
    subscriptions_queue: Vec<(
        entity::subscription_queue::Model,
        Option<entity::subscription_queue_result::Model>,
    )>,
    video_queue: Vec<(
        entity::video_queue::Model,
        Option<video_queue_result::Model>,
    )>,
    known_channels: Vec<entity::known_channels::Model>,
    css: String,
}

pub async fn dashboard(
    State(database): State<DatabaseConnection>,
) -> Result<Html<String>, InternalServerError<DbErr>> {
    Ok(Html(
        Dashboard {
            oauth_token: OAuth::get_token(&database)
                .await
                .map_err(InternalServerError)?,
            subscriptions_queue: entity::subscription_queue::Entity::find()
                .find_also_related(entity::subscription_queue_result::Entity)
                .all(&database)
                .await
                .map_err(InternalServerError)?,
            video_queue: entity::video_queue::Entity::find()
                .find_also_related(entity::video_queue_result::Entity)
                .all(&database)
                .await
                .map_err(InternalServerError)?,
            known_channels: entity::known_channels::Entity::find()
                .all(&database)
                .await
                .map_err(InternalServerError)?,
            css: tokio::fs::read_to_string("./static/styles.css")
                .await
                .map_err(|e| DbErr::Custom(e.to_string()))
                .map_err(InternalServerError)?,
        }
        .render()
        .map_err(|e| DbErr::Custom(e.to_string()))
        .map_err(InternalServerError)?,
    ))
}
