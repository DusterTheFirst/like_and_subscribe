use eframe::egui::{
    self, Button, CentralPanel, Color32, Image, ProgressBar, Rect, ScrollArea, TextStyle,
    UiBuilder, Vec2, Vec2b,
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
    excluded_languages: HashSet<String>,
}

impl AppUi {
    pub fn new(auth: OAuthManager, channel_discovery: ChannelDiscovery) -> Self {
        AppUi {
            auth,
            channel_discovery,

            table_cache: Default::default(),

            show_only_new: false,
            excluded_languages: HashSet::new(),
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

            let (channels, videos, playlist_items, languages) = self
                .channel_discovery
                .observe_state(|channel_discovery, state| match state {
                    ChannelDiscoveryState::Idle(idle) => {
                        if ui.button("Discover Channels").clicked() {
                            idle.start(channel_discovery, access_token);
                        };
                        (
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
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::Discovered(discovered) => {
                        ui.horizontal(|ui| {
                            if ui.button("Find uploads until date").clicked() {
                                discovered.find_uploads(
                                    channel_discovery,
                                    channel_discovery.last_seen_video,
                                    access_token,
                                );
                            }
                        });

                        (
                            discovered.channels.clone(),
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
                            finding_uploads.uploads.clone(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FoundUploads(found_uploads) => {
                        if ui.button("Filter by playlist").clicked() {
                            found_uploads.find_playlist_items(channel_discovery, access_token);
                        }

                        (
                            found_uploads.channels.clone(),
                            found_uploads.uploads.clone(),
                            HashSet::new(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FindingPlaylistItems(finding_playlist_items) => {
                        match finding_playlist_items.total_playlist_items {
                            Some(total) => {
                                let items = finding_playlist_items.playlist_items.len();

                                ui.add(
                                    ProgressBar::new((items as f32) / (total as f32))
                                        .text(format!("{items}/{total}")),
                                );
                            }
                            None => {
                                ui.add(ProgressBar::new(0.0));
                            }
                        }

                        (
                            finding_playlist_items.channels.clone(),
                            finding_playlist_items.uploads.clone(),
                            finding_playlist_items.playlist_items.clone(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::FoundPlaylistItems(found_playlist_items) => {
                        if ui.button("Determine languages").clicked() {
                            found_playlist_items
                                .determine_video_languages(channel_discovery, access_token);
                        }

                        (
                            found_playlist_items.channels.clone(),
                            found_playlist_items.uploads.clone(),
                            found_playlist_items.playlist_items.clone(),
                            HashMap::new(),
                        )
                    }
                    ChannelDiscoveryState::DeterminingLanguages(determining_languages) => {
                        let total_videos = determining_languages
                            .uploads
                            .values()
                            .flat_map(|v| {
                                v.iter().filter(|video| {
                                    video.published_at > channel_discovery.last_seen_video
                                })
                            })
                            .count();
                        let videos_with_language =
                            determining_languages.languages.values().flatten().count();

                        ui.add(
                            ProgressBar::new((videos_with_language as f32) / (total_videos as f32))
                                .text(format!("{videos_with_language}/{total_videos}")),
                        );

                        (
                            determining_languages.channels.clone(),
                            determining_languages.uploads.clone(),
                            determining_languages.playlist_items.clone(),
                            determining_languages.languages.clone(),
                        )
                    }
                    ChannelDiscoveryState::DeterminedLanguages(determined_languages) => {
                        if ui.button("Add new to playlist").clicked() {
                            todo!()
                        }

                        (
                            determined_languages.channels.clone(),
                            determined_languages.uploads.clone(),
                            determined_languages.playlist_items.clone(),
                            determined_languages.languages.clone(),
                        )
                    }
                });

            ui.horizontal_wrapped(|ui| {
                ui.label("Channels: ");
                ui.monospace(channels.len().to_string());
                ui.separator();

                ui.label("Channels with uploads: ");
                ui.monospace(videos.len().to_string());
                ui.separator();

                ui.label("Videos: ");
                ui.monospace(videos.values().map(Vec::len).sum::<usize>().to_string());
                ui.separator();

                ui.label("New videos: ");
                ui.monospace(
                    videos
                        .values()
                        .flat_map(|v| {
                            v.iter().filter(|video| {
                                video.published_at > self.channel_discovery.last_seen_video
                            })
                        })
                        .count()
                        .to_string(),
                );
                ui.separator();

                ui.label("Playlist videos: ");
                ui.monospace(playlist_items.len().to_string());
                ui.separator();

                ui.label("New videos not in playlist: ");
                ui.monospace(
                    videos
                        .values()
                        .flat_map(|v| {
                            v.iter()
                                .filter(|video| {
                                    video.published_at > self.channel_discovery.last_seen_video
                                })
                                .filter(|video| !playlist_items.contains(&video.id))
                        })
                        .count()
                        .to_string(),
                );
                ui.separator();

                ui.label("Languages: ");
                for lang in languages.keys() {
                    let selected = self.excluded_languages.contains(lang);
                    if ui
                        .add(Button::selectable(selected, lang).fill(ui.visuals().warn_fg_color))
                        .clicked()
                    {
                        if selected {
                            self.excluded_languages.remove(lang);
                        } else {
                            self.excluded_languages.insert(lang.clone());
                        }
                    };
                }
                ui.separator();

                ui.label("Previous last seen video: ");
                ui.monospace(self.channel_discovery.last_seen_video.to_string());

                if let Some(last_seen) = videos
                    .values()
                    .flat_map(|v| v.iter().map(|video| video.published_at))
                    .max()
                {
                    ui.label("Current last seen video: ");
                    ui.monospace(last_seen.to_string());
                }

                ui.checkbox(&mut self.show_only_new, "Show only new");
            });

            let rows = self.table_cache.process(channels, |channels| {
                tracing::debug!("sort");
                let mut rows = channels
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>();
                rows.sort_unstable_by_key(|(_, m)| m.name.clone());
                rows
            });

            let rows = if self.show_only_new {
                &rows
                    .iter()
                    .filter(|&(channel, _)| {
                        videos
                            .get(channel)
                            .map(|videos| {
                                videos.iter().any(|video| {
                                    video.published_at > self.channel_discovery.last_seen_video
                                })
                            })
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                rows
            };

            // TODO: calculate quota?
            ScrollArea::both().auto_shrink(Vec2b::FALSE).show(ui, |ui| {
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
                                ui.take_available_height();

                                ui.vertical_centered_justified(|ui| {
                                    ui.label(format!("#{i}"));
                                });
                                ui.separator();

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

                                let videos = videos
                                    .get(channel_id)
                                    .map(|v| v.as_slice())
                                    .unwrap_or_default();

                                ui.label(format!("{} videos loaded", videos.len()));

                                ui.separator();

                                for video in videos {
                                    let new =
                                        video.published_at > self.channel_discovery.last_seen_video;

                                    if self.show_only_new && !new {
                                        break;
                                    }

                                    ui.horizontal(|ui| {
                                        ui.label(format!("#{}", video.position));

                                        let thumbnail =
                                            if self.excluded_languages.iter().any(|lang| {
                                                languages.get(lang).unwrap().contains(&video.id)
                                            }) {
                                                ui.visuals_mut().override_text_color =
                                                    Some(Color32::RED);

                                                true
                                            } else if playlist_items.contains(&video.id) {
                                                ui.visuals_mut().override_text_color =
                                                    Some(Color32::GOLD);

                                                true
                                            } else if new {
                                                ui.visuals_mut().override_text_color =
                                                    Some(Color32::GREEN);
                                                true
                                            } else {
                                                false
                                            };

                                        if thumbnail {
                                            ui.add_sized(
                                                Vec2::ONE
                                                    * ui.text_style_height(&TextStyle::Monospace)
                                                    * 3.0,
                                                Image::new(&video.thumbnail),
                                            );
                                        } else {
                                            ui.add_space(
                                                ui.text_style_height(&TextStyle::Monospace) * 3.0,
                                            );
                                        };

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
