use eframe::egui::{
    CentralPanel, ProgressBar, ScrollArea,
    ahash::{HashMap, HashMapExt},
};
use egui_extras::{Column, TableBuilder};
use jiff::tz::TimeZone;

use crate::{
    api::channels::{ChannelDiscovery, ChannelDiscoveryState, ChannelId, ChannelMetadata},
    cache::CacheProcess,
    oauth::{AuthorizationState, Authorize as _, OAuthManager, Refresh as _},
};

pub struct AppUi {
    auth: OAuthManager,

    channel_discovery: ChannelDiscovery,

    table_cache:
        CacheProcess<HashMap<ChannelId, ChannelMetadata>, Vec<(ChannelId, ChannelMetadata)>>,
}

impl AppUi {
    pub fn new(auth: OAuthManager, channel_discovery: ChannelDiscovery) -> Self {
        AppUi {
            auth,
            channel_discovery,

            table_cache: Default::default(),
        }
    }
}

impl eframe::App for AppUi {
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            let access_token = match self.auth.get_state() {
                AuthorizationState::AuthorizedExpired(unauth) => {
                    ui.label("Authorized but expired");
                    if ui.button("Refresh").clicked() {
                        unauth.refresh(&mut self.auth);
                    }
                    None
                }
                AuthorizationState::Unauthorized(unauth) => {
                    ui.label("Unauthorized");
                    if ui.link("Copy Login Link").clicked() {
                        ctx.copy_text(self.auth.get_auth_url().into());
                        unauth.authorize(&mut self.auth);
                    }
                    None
                }
                AuthorizationState::Authorized(auth) => {
                    ui.horizontal(|ui| {
                        ui.label("Authorized with access until");
                        ui.label(
                            auth.expires_at
                                .to_zoned(TimeZone::system())
                                .strftime("%A, %B %d, %Y at %H:%M%P %Q")
                                .to_string(),
                        );
                    });

                    if ui.button("Refresh").clicked() {
                        auth.refresh(&mut self.auth);
                        None
                    } else {
                        Some(auth.access_token)
                    }
                }
                AuthorizationState::Authorizing => {
                    ui.label("Authorizing....");
                    ui.spinner();
                    if ui.link("Copy Login Link").clicked() {
                        ctx.copy_text(self.auth.get_auth_url().into());
                    }
                    None
                }
                AuthorizationState::Refreshing => {
                    ui.label("Refreshing....");
                    ui.spinner();
                    None
                }
            };

            let Some(access_token) = access_token else {
                ui.centered_and_justified(|ui| {
                    ui.label("Token expired, please refresh or authorize")
                });
                return;
            };

            ui.separator();

            let (channels, playlist, videos) =
                self.channel_discovery
                    .observe_state(|channel_discovery, state| match state {
                        ChannelDiscoveryState::Idle(idle) => {
                            if ui.button("Discover Channels").clicked() {
                                idle.start(channel_discovery, access_token);
                            };
                            (HashMap::new(), HashMap::new(), HashMap::new())
                        }
                        ChannelDiscoveryState::Discovering(discovering) => {
                            match discovering.total_channel_count {
                                Some(total) => {
                                    ui.add(
                                        ProgressBar::new(
                                            (discovering.channels.len() as f32) / (total as f32),
                                        )
                                        .show_percentage(),
                                    );
                                }
                                None => {
                                    ui.add(ProgressBar::new(0.0));
                                }
                            }
                            (discovering.channels.clone(), HashMap::new(), HashMap::new())
                        }
                        ChannelDiscoveryState::Discovered(discovered) => {
                            if ui.button("Elaborate Channels").clicked() {
                                discovered.elaborate(channel_discovery, access_token);
                            }

                            (discovered.channels.clone(), HashMap::new(), HashMap::new())
                        }
                        ChannelDiscoveryState::Elaborating(elaborating) => {
                            ui.add(
                                ProgressBar::new(
                                    (elaborating.elaboration.len() as f32)
                                        / (elaborating.channels.len() as f32),
                                )
                                .show_percentage(),
                            );

                            (
                                elaborating.channels.clone(),
                                elaborating.elaboration.clone(),
                                HashMap::new(),
                            )
                        }
                        ChannelDiscoveryState::Elaborated(elaborated) => {
                            if ui.button("Find new uploads").clicked() {
                                elaborated.find_uploads(channel_discovery, access_token);
                            }

                            (
                                elaborated.channels.clone(),
                                elaborated.elaboration.clone(),
                                HashMap::new(),
                            )
                        }
                        ChannelDiscoveryState::FindingUploads(finding_uploads) => {
                            ui.add(
                                ProgressBar::new(
                                    (finding_uploads.uploads.len() as f32)
                                        / (finding_uploads.channels.len() as f32),
                                )
                                .show_percentage(),
                            );

                            (
                                finding_uploads.channels.clone(),
                                finding_uploads.elaboration.clone(),
                                finding_uploads.uploads.clone(),
                            )
                        }
                        ChannelDiscoveryState::FoundUploads(found_uploads) => {
                            ui.button("Add new uploads to playlist");

                            (
                                found_uploads.channels.clone(),
                                found_uploads.elaboration.clone(),
                                found_uploads.uploads.clone(),
                            )
                        }
                    });

            let rows = self.table_cache.process(channels, |channels| {
                tracing::debug!("sort");
                let mut rows = channels
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>();
                rows.sort_unstable_by_key(|(k, m)| m.name.clone());
                rows
            });

            // TODO: calculate quota?

            TableBuilder::new(ui)
                .column(Column::exact(200.0))
                .column(Column::exact(150.0))
                .column(Column::exact(150.0))
                .column(Column::exact(200.0))
                .column(Column::remainder())
                .header(20.0, |mut row| {
                    row.col(|ui| {
                        ui.heading("Channel Id");
                    });
                    row.col(|ui| {
                        ui.heading("Channel Name");
                    });
                    row.col(|ui| {
                        ui.heading("Channel Profile");
                    });
                    row.col(|ui| {
                        ui.heading("Channel Playlist");
                    });
                    row.col(|ui| {
                        ui.heading("Channel Videos");
                    });
                })
                .body(|body| {
                    body.rows(60.0, rows.len(), |mut row| {
                        let (
                            channel_id,
                            ChannelMetadata {
                                name,
                                profile_picture,
                            },
                        ) = &rows[row.index()];
                        let channel_playlist = playlist.get(channel_id);

                        row.col(|ui| {
                            ui.monospace(channel_id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(name);
                        });
                        row.col(|ui| {
                            ui.image(profile_picture);
                        });
                        row.col(|ui| {
                            if let Some(playlist) = channel_playlist {
                                ui.monospace(playlist.to_string());
                            }
                        });
                        row.col(|ui| {
                            ScrollArea::horizontal().show(ui, |ui| {
                                let videos = videos
                                    .get(channel_id)
                                    .map(|v| v.as_slice())
                                    .unwrap_or_default();

                                ui.horizontal(|ui| for video in videos {
                                    ui.label(&video.title);
                                    ui.label(video.position.to_string());
                                    ui.label(video.published_at.to_zoned(TimeZone::system()).strftime("%A, %B %d, %Y at %H:%M%P %Q").to_string());
                                    ui.image(&video.thumbnail);
                                    ui.separator();
                                });
                            });
                        });
                    });
                });
        });
    }
}
