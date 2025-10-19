use std::convert::identity;

use eframe::egui::{
    self, CentralPanel, Color32, Image, ProgressBar, Rect, ScrollArea, TextStyle, UiBuilder, Vec2,
    ahash::{HashMap, HashMapExt, HashSet, HashSetExt},
};
use jiff::tz::TimeZone;

use crate::{
    cache::CacheProcess,
    discovery::{ChannelDiscovery, ChannelDiscoveryState, ChannelId, ChannelMetadata},
    oauth::{AuthorizationState, Authorize as _, OAuthManager, Refresh as _},
};

pub struct AppUi {
    auth: OAuthManager,

    channel_discovery: ChannelDiscovery,

    table_cache:
        CacheProcess<HashMap<ChannelId, ChannelMetadata>, Vec<(ChannelId, ChannelMetadata)>>,

    show_only_new: bool,
}

impl AppUi {
    pub fn new(auth: OAuthManager, channel_discovery: ChannelDiscovery) -> Self {
        AppUi {
            auth,
            channel_discovery,

            table_cache: Default::default(),

            show_only_new: false,
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

            let (channels, playlist, videos, date_filtered_videos, is_short) = self
                .channel_discovery
                .observe_state(|channel_discovery, state| match state {
                    ChannelDiscoveryState::Idle(idle) => {
                        if ui.button("Discover Channels").clicked() {
                            idle.start(channel_discovery, access_token);
                        };
                        (
                            HashMap::new(),
                            HashMap::new(),
                            HashMap::new(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::Discovering(discovering) => {
                        match discovering.total_channel_count {
                            Some(total) => {
                                let channels = discovering.channels.len();

                                ui.add(
                                    ProgressBar::new((channels as f32) / (total as f32))
                                        .text(format!("{channels}/{total}")),
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
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::Discovered(discovered) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Channels: {}", discovered.channels.len()));

                            if ui.button("Elaborate Channels").clicked() {
                                discovered.elaborate(channel_discovery, access_token);
                            }
                        });

                        (
                            discovered.channels.clone(),
                            HashMap::new(),
                            HashMap::new(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::Elaborating(elaborating) => {
                        let elaboration = elaborating.elaboration.len();
                        let channels = elaborating.channels.len();
                        ui.add(
                            ProgressBar::new((elaboration as f32) / (channels as f32))
                                .text(format!("{elaboration}/{channels}")),
                        );

                        (
                            elaborating.channels.clone(),
                            elaborating.elaboration.clone(),
                            HashMap::new(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::Elaborated(elaborated) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Channels: {}", elaborated.channels.len()));
                            ui.label(format!("Elaborations: {}", elaborated.elaboration.len()));

                            if ui.button("Find new uploads").clicked() {
                                elaborated.find_uploads(channel_discovery, access_token);
                            }
                        });

                        (
                            elaborated.channels.clone(),
                            elaborated.elaboration.clone(),
                            HashMap::new(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FindingUploads(finding_uploads) => {
                        let uploads = finding_uploads.uploads.len();
                        let channels = finding_uploads.channels.len();

                        ui.add(
                            ProgressBar::new((uploads as f32) / (channels as f32))
                                .text(format!("{uploads}/{channels}")),
                        );

                        (
                            finding_uploads.channels.clone(),
                            finding_uploads.elaboration.clone(),
                            finding_uploads.uploads.clone(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FoundUploads(found_uploads) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Channels: {}", found_uploads.channels.len()));
                            ui.label(format!("Elaborations: {}", found_uploads.elaboration.len()));
                            ui.label(format!(
                                "Channels with uploads: {}",
                                found_uploads.uploads.len()
                            ));
                            ui.label(format!(
                                "Total videos: {}",
                                found_uploads.uploads.values().map(Vec::len).sum::<usize>()
                            ));

                            ui.label("Filter: ");
                            ui.monospace(channel_discovery.last_seen_video.to_string());

                            if ui.button("Filter by date").clicked() {
                                found_uploads
                                    .filter(channel_discovery, channel_discovery.last_seen_video);
                            }
                        });

                        (
                            found_uploads.channels.clone(),
                            found_uploads.elaboration.clone(),
                            found_uploads.uploads.clone(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FilteredByDate(filtered) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Channels: {}", filtered.channels.len()));
                            ui.label(format!("Elaborations: {}", filtered.elaboration.len()));
                            ui.label(format!("Channels with uploads: {}", filtered.uploads.len()));
                            ui.label(format!(
                                "Total videos: {}",
                                filtered.uploads.values().map(Vec::len).sum::<usize>()
                            ));
                            ui.label(format!(
                                "Total new videos: {}",
                                filtered.date_filtered_videos.len()
                            ));

                            if ui.button("Filter by shorts").clicked() {
                                filtered.filter(channel_discovery, access_token);
                            }
                        });

                        (
                            filtered.channels.clone(),
                            filtered.elaboration.clone(),
                            filtered.uploads.clone(),
                            filtered.date_filtered_videos.clone(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FilteringByShorts(filtering_by_shorts) => {
                        let total = filtering_by_shorts.date_filtered_videos.len();
                        let shorts = filtering_by_shorts.is_short.len();

                        ui.add(
                            ProgressBar::new((shorts as f32) / (total as f32))
                                .text(format!("{shorts}/{total}")),
                        );

                        (
                            filtering_by_shorts.channels.clone(),
                            filtering_by_shorts.elaboration.clone(),
                            filtering_by_shorts.uploads.clone(),
                            filtering_by_shorts.date_filtered_videos.clone(),
                            filtering_by_shorts.is_short.clone(),
                        )
                    }
                    ChannelDiscoveryState::FilteredByShorts(filtered_by_shorts) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Channels: {}", filtered_by_shorts.channels.len()));
                            ui.label(format!(
                                "Elaborations: {}",
                                filtered_by_shorts.elaboration.len()
                            ));
                            ui.label(format!(
                                "Channels with uploads: {}",
                                filtered_by_shorts.uploads.len()
                            ));
                            ui.label(format!(
                                "Total videos: {}",
                                filtered_by_shorts
                                    .uploads
                                    .values()
                                    .map(Vec::len)
                                    .sum::<usize>()
                            ));
                            ui.label(format!(
                                "Total new videos: {}",
                                filtered_by_shorts.date_filtered_videos.len()
                            ));
                            ui.label(format!(
                                "Total short videos: {}",
                                filtered_by_shorts.is_short.values().filter(|x| **x).count()
                            ));
                            ui.label(format!(
                                "Total non-short videos: {}",
                                filtered_by_shorts
                                    .is_short
                                    .values()
                                    .filter(|x| !**x)
                                    .count()
                            ));

                            ui.button("Filter if in playlist");
                            ui.button("Add new uploads to playlist");
                        });

                        (
                            filtered_by_shorts.channels.clone(),
                            filtered_by_shorts.elaboration.clone(),
                            filtered_by_shorts.uploads.clone(),
                            filtered_by_shorts.date_filtered_videos.clone(),
                            filtered_by_shorts.is_short.clone(),
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

            ui.checkbox(&mut self.show_only_new, "Show only new");

            let rows = if self.show_only_new {
                &rows
                    .iter()
                    .filter(|&(channel, _)| {
                        videos
                            .get(channel)
                            .map(|videos| {
                                videos
                                    .iter()
                                    .any(|video| date_filtered_videos.contains(&video.id))
                            })
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                rows
            };

            // TODO: calculate quota?

            ScrollArea::both().show(ui, |ui| {
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
                                        if let Some(true) = is_short.get(&video.id) {
                                            ui.visuals_mut().override_text_color =
                                                Some(Color32::RED);
                                        } else if date_filtered_videos.contains(&video.id) {
                                            ui.visuals_mut().override_text_color =
                                                Some(Color32::GREEN);
                                        }

                                        ui.label(format!("#{}", video.position));

                                        if date_filtered_videos.contains(&video.id) {
                                            ui.add_sized(
                                                Vec2::ONE
                                                    * ui.text_style_height(&TextStyle::Monospace)
                                                    * 3.0,
                                                Image::new(&video.thumbnail),
                                            );
                                        }

                                        ui.vertical(|ui| {
                                            ui.label(&video.title);
                                            ui.label(video.id.to_string());
                                            ui.label(video.published_at.to_string());
                                            ui.add_space(10.0);
                                        })
                                    });
                                }
                            });

                            ui.separator();
                        }
                    },
                );
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
