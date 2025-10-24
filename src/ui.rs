use std::collections::BTreeSet;

use eframe::egui::{
    Button, CentralPanel, Color32, Image, ProgressBar, ScrollArea, TextStyle, Vec2, Vec2b,
};
use jiff::tz::TimeZone;

use crate::{
    discovery::{ChannelDiscovery, ChannelDiscoveryState},
    oauth::{AuthorizationState, Authorize as _, OAuthManager, Refresh as _},
};

pub struct AppUi {
    auth: OAuthManager,

    channel_discovery: ChannelDiscovery,

    filter_new: bool,
    filter_playlist: bool,
    filter_language: bool,
}

impl AppUi {
    pub fn new(auth: OAuthManager, channel_discovery: ChannelDiscovery) -> Self {
        AppUi {
            auth,
            channel_discovery,

            filter_new: false,
            filter_playlist: false,
            filter_language: false,
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
                                ui.label(format!("Last update: {}", state.last_update));
                                if ui.button("Find uploads until date").clicked() {
                                    state.find_uploads(channel_discovery, access_token);
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
                                                    None,
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::FoundUploads(state) => {
                            ui.horizontal(|ui| {
                                ui.label(format!("Last update: {}", state.last_update));
                                if ui.button("Filter by upload date").clicked() {
                                    state.filter(channel_discovery);
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
                                                    false,
                                                    false,
                                                    None,
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
                                                    None,
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
                                                    None,
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
                                                    None,
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                        ChannelDiscoveryState::DeterminingLanguages(state) => {
                            let total_videos = state.new_videos.len();
                            let videos_with_language = state.video_languages.len();

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
                                                    state
                                                        .video_languages
                                                        .get(video)
                                                        .map(|lang| (lang.clone(), false)),
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
                                for lang in state.video_languages.values().collect::<BTreeSet<_>>()
                                {
                                    let selected = state.excluded_languages.contains(lang);
                                    let count = state
                                        .video_languages
                                        .values()
                                        .filter(|l| l == &lang)
                                        .count();

                                    if ui
                                        .add(
                                            Button::selectable(
                                                selected,
                                                format!("{lang} ({count})"),
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

                                if ui.button("Add to playlist").clicked() {
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
                                                    state.video_languages.get(video).map(|lang| {
                                                        (
                                                            lang.clone(),
                                                            state.excluded_languages.contains(lang),
                                                        )
                                                    }),
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .collect()
                        }
                    });

            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.filter_new, "Filter by new");
                ui.checkbox(&mut self.filter_playlist, "Filter by playlist");
                ui.checkbox(&mut self.filter_language, "Filter by language");
            });

            display_channels.sort_unstable_by_key(|(_, m, _)| m.name.clone());

            if self.filter_new {
                // Remove channels with no new
                display_channels.retain_mut(|(_, _, videos)| {
                    videos.retain(|(_, _, is_new, in_playlist, language)| *is_new);

                    !videos.is_empty()
                });
            }

            if self.filter_playlist {
                display_channels.retain_mut(|(_, _, videos)| {
                    videos.retain(|(_, _, is_new, in_playlist, language)| !*in_playlist);

                    !videos.is_empty()
                });
            }

            if self.filter_language {
                display_channels.retain_mut(|(_, _, videos)| {
                    videos.retain(|(_, _, is_new, in_playlist, language)| {
                        language.as_ref().is_some_and(|(_, exclude)| !*exclude)
                    });

                    !videos.is_empty()
                });
            }

            // TODO: calculate quota?
            ScrollArea::both().auto_shrink(Vec2b::FALSE).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (channel_id, channel_metadata, videos) in display_channels.iter() {
                        ui.vertical(|ui| {
                            ui.set_width(300.0);
                            ui.take_available_height();

                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    Vec2::ONE * ui.text_style_height(&TextStyle::Monospace) * 3.0,
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

                                let filter_new_count = videos
                                    .iter()
                                    .filter(|(_, _, is_new, in_playlist, lang)| *is_new)
                                    .count();
                                let filter_playlist_count = videos
                                    .iter()
                                    .filter(|(_, _, is_new, in_playlist, lang)| *in_playlist)
                                    .count();
                                let filter_lang_count = videos
                                    .iter()
                                    .filter(|(_, _, is_new, in_playlist, lang)| {
                                        lang.as_ref().is_some_and(|(_, exclude)| *exclude)
                                    })
                                    .count();

                                ui.label(format!(
                                    "{}/{} new videos",
                                    filter_new_count,
                                    videos.len()
                                ));
                                ui.label(format!(
                                    "{}/{} videos in playlist",
                                    filter_playlist_count, filter_new_count
                                ));
                                ui.label(format!("{} videos excluded lang", filter_lang_count,));
                            });

                            ui.separator();

                            for (video, meta, is_new, in_playlist, lang) in videos {
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
                                            if let Some((lang, exclude)) = lang {
                                                ui.colored_label(
                                                    if *exclude {
                                                        Color32::RED
                                                    } else {
                                                        Color32::PURPLE
                                                    },
                                                    lang,
                                                );
                                            }
                                        });
                                        ui.add_space(10.0);
                                    })
                                });
                            }
                        });

                        ui.separator();
                    }
                });
            });
        });
    }
}
