use eframe::egui::{Button, CentralPanel};
use jiff::{SignedDuration, Timestamp, Zoned, civil::DateTime, tz::TimeZone};

use crate::oauth::{Authenticate as _, AuthenticationManager, AuthenticationState, Refresh as _};

pub struct AppUi {
    auth: AuthenticationManager,
}

impl AppUi {
    pub fn new(auth: AuthenticationManager) -> Self {
        AppUi { auth }
    }
}

impl eframe::App for AppUi {
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            let access_token = match self.auth.get_state(ctx) {
                AuthenticationState::UnauthenticatedRefresh(unauth) => {
                    ui.label("Unauthenticated");
                    if ui.button("Refresh").clicked() {
                        unauth.refresh(&mut self.auth, ctx.clone());
                    }
                    None
                }
                AuthenticationState::Unauthenticated(unauth) => {
                    ui.label("Unauthenticated");
                    if ui.link("Copy Login Link").clicked() {
                        ctx.copy_text(self.auth.get_auth_url().into());
                        unauth.authenticate(&mut self.auth, ctx.clone());
                    }
                    None
                }
                AuthenticationState::AuthenticatedRefresh(auth) => {
                    ui.label("Authenticated with refresh token until");

                    ui.label(
                        auth.expires_at
                            .to_zoned(TimeZone::system())
                            .strftime("%A, %B %d, %Y at %H:%M%P %Q")
                            .to_string(),
                    );

                    Some(auth.access_token)
                }
                AuthenticationState::Authenticated(auth) => {
                    ui.label("Authenticated until");
                    ui.label(
                        auth.expires_at
                            .to_zoned(TimeZone::system())
                            .strftime("%A, %B %d, %Y at %H:%M%P %Q")
                            .to_string(),
                    );

                    Some(auth.access_token)
                }
                AuthenticationState::Authenticating => {
                    ui.label("Authenticating....");
                    ui.spinner();
                    if ui.link("Copy Login Link").clicked() {
                        ctx.copy_text(self.auth.get_auth_url().into());
                    }
                    None
                }
                AuthenticationState::Refreshing => {
                    ui.label("Refreshing....");
                    ui.spinner();
                    None
                }
            };

            let Some(access_token) = access_token else {
                ui.centered_and_justified(|ui| ui.label("Log in first please"));
                return;
            };

            ui.separator();

            ui.button("Discover Channels");
            ui.button("Find new uploads");
            ui.button("Add new uploads to playlist");
        });
    }
}
