use std::{ffi::OsString, path::PathBuf};

use eframe::NativeOptions;
use envconfig::Envconfig;
use tracing_error::ErrorLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{database::Database, oauth::AuthenticationManager, ui::AppUi};

// mod actor;
mod database;
mod oauth;
mod ui;

#[derive(Envconfig)]
pub struct Config {
    #[envconfig(from = "GOOGLE_CLIENT_ID")]
    pub google_client_id: String,
    #[envconfig(from = "GOOGLE_CLIENT_SECRET")]
    pub google_client_secret: String,

    #[envconfig(from = "YOUTUBE_PLAYLIST_ID")]
    pub youtube_playlist_id: String,

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
        Box::new(move |_cc| {
            Ok(Box::new(AppUi::new(AuthenticationManager::new(
                oauth2::ClientId::new(config.google_client_id),
                oauth2::ClientSecret::new(config.google_client_secret),
                database.clone(),
            ))))
        }),
    )

    // let token_manager = TokenManager::init(
    //     database.clone(),
    //     google_client_id,
    //     google_client_secret,
    //     config.hostname.clone(),
    //     email_send_tx,
    // )
    // .await
    // .wrap_err("unable to initialize the token manager")?;

    // // Oauth service
    // let mut web_server_task = tasks.spawn(web_server(shutdown.clone(), token_manager.clone()));

    // // Authenticated services
    // let mut subscription_task = tasks.spawn(playlist_updater(
    //     shutdown.clone(),
    //     database.clone(),
    //     client.clone(),
    //     token_manager,
    //     Arc::from(config.youtube_playlist_id),
    // ));

    // Ok(())
}
