use eframe::egui::{
    self, Button, CentralPanel, Color32, Image, ProgressBar, Rect, ScrollArea, TextStyle,
    UiBuilder, Vec2, Vec2b,
};
use jiff::tz::TimeZone;

use crate::{
    discovery::{ChannelDiscovery, ChannelDiscoveryState},
    oauth::{AuthorizationState, Authorize as _, OAuthManager, Refresh as _},
};

pub struct AppUi {
    auth: OAuthManager,

    channel_discovery: ChannelDiscovery,

    show_only_new: bool,
}

impl AppUi {
    pub fn new(auth: OAuthManager, channel_discovery: ChannelDiscovery) -> Self {
        AppUi {
            auth,
            channel_discovery,

            show_only_new: false,
        }
    }
}

impl eframe::App for AppUi {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
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

            let mut display_channels =
                self.channel_discovery
                    .observe_state(|channel_discovery, state| match state {
                        ChannelDiscoveryState::Idle(state) => {
                            if ui.button("Discover Channels").clicked() {
                                state.start(channel_discovery, access_token);
                            };

                            Vec::new()
                        }
                        ChannelDiscoveryState::Discovering(state) => {
                            match state.total_channel_count {
                                Some(total) => {
                                    let channels = state.channels.len();

                                    ui.add(
                                        ProgressBar::new((channels as f32) / (total as f32))
                                            .text(format!("{channels}/{total}")),
                                    );
                                }
                                None => {
                                    ui.add(ProgressBar::new(0.0));
                                }
                            }

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| (channel.clone(), meta.clone(), Vec::new()))
                                .collect()
                        }
                        ChannelDiscoveryState::Discovered(state) => {
                            ui.horizontal(|ui| {
                                if ui.button("Find uploads until date").clicked() {
                                    state.find_uploads(
                                        channel_discovery,
                                        channel_discovery.last_seen_video,
                                        access_token,
                                    );
                                }
                            });

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| (channel.clone(), meta.clone(), Vec::new()))
                                .collect()
                        }
                        ChannelDiscoveryState::FindingUploads(state) => {
                            let uploads = state.uploads.len();
                            let channels = state.channels.len();

                            ui.add(
                                ProgressBar::new((uploads as f32) / (channels as f32))
                                    .text(format!("{uploads}/{channels}")),
                            );

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    false,
                                                    false,
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::FoundUploads(state) => {
                            if ui.button("Filter by upload date").clicked() {
                                state.filter(channel_discovery, channel_discovery.last_seen_video);
                            }

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    false,
                                                    false,
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::FilteredByDate(state) => {
                            if ui.button("Find playlist items").clicked() {
                                state.find_playlist_items(channel_discovery, access_token);
                            }

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    state.new_videos.contains(video),
                                                    false,
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::FindingPlaylistItems(state) => {
                            match state.total_playlist_items {
                                Some(total) => {
                                    let items = state.playlist_items.len();

                                    ui.add(
                                        ProgressBar::new((items as f32) / (total as f32))
                                            .text(format!("{items}/{total}")),
                                    );
                                }
                                None => {
                                    ui.add(ProgressBar::new(0.0));
                                }
                            }

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    state.new_videos.contains(video),
                                                    state.playlist_items.contains(video),
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::FoundPlaylistItems(state) => {
                            if ui.button("Determine languages").clicked() {
                                state.determine_video_languages(channel_discovery, access_token);
                            }

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    state.new_videos.contains(video),
                                                    state.playlist_items.contains(video),
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::DeterminingLanguages(state) => {
                            let total_videos = state.new_videos.len();
                            let videos_with_language = state.languages.values().flatten().count();

                            ui.add(
                                ProgressBar::new(
                                    (videos_with_language as f32) / (total_videos as f32),
                                )
                                .text(format!("{videos_with_language}/{total_videos}")),
                            );

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    state.new_videos.contains(video),
                                                    state.playlist_items.contains(video),
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::DeterminedLanguages(state) => {
                            ui.horizontal(|ui| {
                                ui.label("Languages: ");
                                for (lang, vids) in state.languages.iter() {
                                    let selected = state.excluded_languages.contains(lang);
                                    if ui
                                        .add(
                                            Button::selectable(
                                                selected,
                                                format!("{lang} ({})", vids.len()),
                                            )
                                            .fill(ui.visuals().warn_fg_color),
                                        )
                                        .clicked()
                                    {
                                        if selected {
                                            state.excluded_languages.remove(lang);
                                        } else {
                                            state.excluded_languages.insert(lang.clone());
                                        }
                                    };
                                }
                                ui.separator();

                                if ui.button("Exclude").clicked() {
                                    todo!()
                                }

                                if ui.button("Add new to playlist").clicked() {
                                    todo!()
                                }
                            });

                            state
                                .channels
                                .iter()
                                .map(|(channel, meta)| {
                                    (
                                        channel.clone(),
                                        meta.clone(),
                                        state
                                            .uploads
                                            .get(channel)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default()
                                            .iter()
                                            .map(|video| {
                                                (
                                                    video.clone(),
                                                    state.videos[video].clone(),
                                                    state.new_videos.contains(video),
                                                    state.playlist_items.contains(video),
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                    });

            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.show_only_new, "Show only new");
            });

            display_channels.sort_unstable_by_key(|(_, m, _)| m.name.clone());

            if self.show_only_new {
                // Remove channels with no new
                display_channels.retain(|(channel, meta, videos)| {
                    videos.iter().any(|(_, _, is_new, in_playlist)| *is_new)
                });
            }

            // TODO: calculate quota?
            ScrollArea::both().auto_shrink(Vec2b::FALSE).show(ui, |ui| {
                show_columns(
                    ScrollArea::horizontal(),
                    ui,
                    300.0,
                    display_channels.len(),
                    |ui, range| {
                        for (i, (channel_id, channel_metadata, videos)) in
                            display_channels[range.clone()].iter().enumerate()
                        {
                            let i = i + range.start;

                            ui.vertical(|ui| {
                                ui.set_width(300.0);
                                ui.take_available_height();

                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        Vec2::ONE
                                            * ui.text_style_height(&TextStyle::Monospace)
                                            * 3.0,
                                        Image::new(&channel_metadata.profile_picture),
                                    );

                                    ui.vertical(|ui| {
                                        ui.label(&channel_metadata.name);
                                        ui.monospace(channel_id.as_ref());
                                        ui.monospace(channel_id.playlist_long_form().as_ref());
                                    })
                                });

                                ui.separator();

                                ui.horizontal(|ui| {
                                    ui.label(format!("{} videos loaded", videos.len()));

                                    let new_videos = videos
                                        .iter()
                                        .filter(|(_, _, is_new, in_playlist)| *is_new)
                                        .count();
                                    let in_playlist_videos = videos
                                        .iter()
                                        .filter(|(_, _, is_new, in_playlist)| *in_playlist)
                                        .count();

                                    ui.label(format!("{} new videos", new_videos));
                                    ui.label(format!("{} videos in playlist", in_playlist_videos));
                                });

                                ui.separator();

                                for (video, meta, is_new, in_playlist) in videos {
                                    if self.show_only_new && !is_new {
                                        break;
                                    }

                                    ui.horizontal(|ui| {
                                        ui.label(format!("#{}", meta.position));

                                        if *in_playlist || *is_new {
                                            ui.add_sized(
                                                Vec2::ONE
                                                    * ui.text_style_height(&TextStyle::Monospace)
                                                    * 4.0,
                                                Image::new(&meta.thumbnail),
                                            );
                                        } else {
                                            ui.add_space(
                                                ui.text_style_height(&TextStyle::Monospace) * 4.0,
                                            );
                                        };

                                        ui.vertical(|ui| {
                                            ui.label(&meta.title);
                                            ui.label(video.to_string());
                                            ui.label(meta.published_at.to_string());

                                            ui.horizontal(|ui| {
                                                if *in_playlist {
                                                    ui.colored_label(Color32::GOLD, "PLAYLIST");
                                                }
                                                if *is_new {
                                                    ui.colored_label(Color32::GREEN, "NEW");
                                                }
                                            });
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
