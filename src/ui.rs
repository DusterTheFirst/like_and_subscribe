use eframe::egui::{CentralPanel, ProgressBar, ahash::HashMap};
use egui_extras::{Column, Table, TableBuilder};
use jiff::tz::TimeZone;

use crate::{
    api::channels::{ChannelDiscovery, ChannelDiscoveryState, ChannelMetadata},
    oauth::{AuthorizationState, Authorize as _, OAuthManager, Refresh as _},
};

pub struct AppUi {
    auth: OAuthManager,

    channel_discovery: ChannelDiscovery,
}

impl AppUi {
    pub fn new(auth: OAuthManager, channel_discovery: ChannelDiscovery) -> Self {
        AppUi {
            auth,
            channel_discovery,
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

            ui.button("Find new uploads");
            ui.button("Add new uploads to playlist");

            let channels = match self.channel_discovery.state() {
                ChannelDiscoveryState::Idle(mut idle) => {
                    if ui.button("Discover Channels").clicked() {
                        idle.start(access_token);
                    };
                    &HashMap::default()
                }
                ChannelDiscoveryState::Discovering {
                    discovered_channels,
                    total_channel_count,
                } => {
                    match total_channel_count {
                        Some(total) => {
                            ui.add(
                                ProgressBar::new(
                                    (discovered_channels.len() as f32) / (total as f32),
                                )
                                .show_percentage(),
                            );
                        }
                        None => {
                            ui.add(ProgressBar::new(0.0));
                        }
                    }
                    discovered_channels
                }
                ChannelDiscoveryState::Complete {
                    discovered_channels,
                } => discovered_channels,
            };

            let mut rows: Vec<_> = channels.iter().collect();
            // FIXME: memoize
            rows.sort_unstable_by_key(|(k, m)| &m.name);

            // TODO: calculate quota?

            TableBuilder::new(ui)
                .column(Column::exact(150.0))
                .column(Column::exact(150.0).resizable(true))
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
                })
                .body(|body| {
                    body.rows(40.0, rows.len(), |mut row| {
                        let (
                            channel_id,
                            ChannelMetadata {
                                name,
                                profile_picture,
                            },
                        ) = rows[row.index()];

                        row.col(|ui| {
                            ui.monospace(channel_id);
                        });
                        row.col(|ui| {
                            ui.label(name);
                        });
                        row.col(|ui| {
                            ui.image(profile_picture);
                        });
                    });
                });
        });
    }
}
