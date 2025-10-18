use eframe::egui::{
    self, CentralPanel, Grid, Image, ProgressBar, Rect, ScrollArea, TextStyle, UiBuilder, Vec2,
    ahash::{HashMap, HashMapExt},
};
use egui_extras::{Column, Size, StripBuilder, TableBuilder};
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

            let (channels, playlist, videos, focus_column) =
                self.channel_discovery
                    .observe_state(|channel_discovery, state| match state {
                        ChannelDiscoveryState::Idle(idle) => {
                            if ui.button("Discover Channels").clicked() {
                                idle.start(channel_discovery, access_token);
                            };
                            (HashMap::new(), HashMap::new(), HashMap::new(), None)
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
                            (
                                discovering.channels.clone(),
                                HashMap::new(),
                                HashMap::new(),
                                None,
                            )
                        }
                        ChannelDiscoveryState::Discovered(discovered) => {
                            if ui.button("Elaborate Channels").clicked() {
                                discovered.elaborate(channel_discovery, access_token);
                            }

                            (
                                discovered.channels.clone(),
                                HashMap::new(),
                                HashMap::new(),
                                None,
                            )
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
                                None,
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
                                None,
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
                                finding_uploads.current_channel.clone(),
                            )
                        }
                        ChannelDiscoveryState::FoundUploads(found_uploads) => {
                            ui.button("Add new uploads to playlist");

                            (
                                found_uploads.channels.clone(),
                                found_uploads.elaboration.clone(),
                                found_uploads.uploads.clone(),
                                None,
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

            ScrollArea::both().show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    show_columns(
                        ScrollArea::horizontal(),
                        ui,
                        300.0,
                        rows.len(),
                        |ui, range| {
                            for (i, (channel_id, channel_metadata)) in
                                rows[range.clone()].iter().enumerate()
                            {
                                let i = i + range.start;

                                ui.vertical(|ui| {
                                    ui.set_width(300.0);
                                    ui.vertical_centered_justified(|ui| {
                                        ui.label(format!("#{i}"));
                                    });
                                    ui.separator();

                                    let channel_playlist = playlist.get(channel_id);

                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            Vec2::ONE
                                                * ui.text_style_height(&TextStyle::Monospace)
                                                * 3.0,
                                            Image::new(&channel_metadata.profile_picture),
                                        );

                                        ui.vertical(|ui| {
                                            ui.label(&channel_metadata.name);
                                            ui.monospace(channel_id.to_string());
                                            if let Some(playlist) = channel_playlist {
                                                ui.monospace(playlist.to_string());
                                            }
                                        })
                                    });

                                    ui.separator();

                                    let videos = videos
                                        .get(channel_id)
                                        .map(|v| v.as_slice())
                                        .unwrap_or_default();

                                    ui.label(format!("{} videos loaded", videos.len()));

                                    ui.separator();

                                    for video in videos {
                                        ui.horizontal(|ui| {
                                            ui.label(format!("#{}", video.position));

                                            // ui.add_sized(
                                            //     Vec2::ONE
                                            //         * ui.text_style_height(&TextStyle::Monospace)
                                            //         * 3.0,
                                            //     Image::new(&video.thumbnail),
                                            // );

                                            ui.vertical(|ui| {
                                                ui.label(&video.title);
                                                ui.label(video.published_at.to_string());
                                            })
                                        });
                                    }
                                });

                                let response = ui.separator();
                                if let Some(focus_channel) = &focus_column
                                    && channel_id == focus_channel
                                {
                                    response.scroll_to_me(None);
                                }
                            }
                        },
                    );
                });
            });
        });
    }
}

fn show_columns(
    scroll_area: ScrollArea,
    ui: &mut egui::Ui,
    item_width_without_spacing: f32,
    total_items: usize,
    add_contents: impl FnOnce(&mut egui::Ui, std::ops::Range<usize>),
) {
    use egui::NumExt as _;

    let spacing = ui.spacing().item_spacing;
    let item_width_with_spacing = item_width_without_spacing + spacing.x;
    scroll_area.show_viewport(ui, |ui, viewport| {
        ui.set_width({
            let total_items_f = total_items as f32;
            let including_last_padding = item_width_with_spacing * total_items_f;
            let width = including_last_padding - spacing.x;
            width.at_least(0.0)
        });

        let min_col = (viewport.min.x / item_width_with_spacing).floor() as usize;
        let max_col = (viewport.max.x / item_width_with_spacing).ceil() as usize + 1;
        let max_col = max_col.at_most(total_items);

        let x_min = ui.max_rect().left() + min_col as f32 * item_width_with_spacing;
        let x_max = ui.max_rect().left() + max_col as f32 * item_width_with_spacing;

        let rect = Rect::from_x_y_ranges(x_min..=x_max, ui.max_rect().y_range());

        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.skip_ahead_auto_ids(min_col);
            ui.horizontal(|ui| {
                add_contents(ui, min_col..max_col);
            });
        });
    });
}
