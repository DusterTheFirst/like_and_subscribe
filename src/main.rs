use std::{ffi::OsString, path::PathBuf};

use eframe::NativeOptions;
use envconfig::Envconfig;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{
    database::Database,
    discovery::{ChannelDiscovery, PlaylistId},
    oauth::OAuthManager,
    ui::AppUi,
};

mod database;
mod discovery;
mod oauth;
mod ui;

#[derive(Envconfig)]
pub struct Config {
    #[envconfig(from = "GOOGLE_CLIENT_ID")]
    pub google_client_id: String,
    #[envconfig(from = "GOOGLE_CLIENT_SECRET")]
    pub google_client_secret: String,

    #[envconfig(from = "YOUTUBE_PLAYLIST_ID")]
    pub youtube_playlist_id: PlaylistId,

    #[envconfig(from = "DATABASE_URL")]
    pub database_url: OsString,
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_file(true)
                .with_line_number(true),
        )
        .with(ErrorLayer::default())
        .with(EnvFilter::from_default_env())
        .init();

    tracing::trace!("a");
    tracing::debug!("a");
    tracing::info!("a");
    tracing::warn!("a");
    tracing::error!("a");

    let config = Config::init_from_env().unwrap();

    let database = Database::create(PathBuf::from(config.database_url));

    eframe::run_native(
        "like_and_subscribe",
        NativeOptions {
            ..Default::default()
        },
        Box::new(move |cc| {
            egui_extras::loaders::install_image_loaders(&cc.egui_ctx);

            Ok(Box::new(AppUi::new(
                OAuthManager::new(
                    oauth2::ClientId::new(config.google_client_id),
                    oauth2::ClientSecret::new(config.google_client_secret),
                    database.clone(),
                    cc.egui_ctx.clone(),
                ),
                ChannelDiscovery::new(config.youtube_playlist_id, cc.egui_ctx.clone()),
            )))
        }),
    )
}
