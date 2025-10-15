use eframe::egui::{CentralPanel, ProgressBar};
use jiff::tz::TimeZone;

use crate::oauth::{AuthorizationState, Authorize as _, OAuthManager, Refresh as _};

pub struct AppUi {
    auth: OAuthManager,
}

impl AppUi {
    pub fn new(auth: OAuthManager) -> Self {
        AppUi { auth }
    }
}

impl eframe::App for AppUi {
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            let is_authorized = match self.auth.get_state() {
                AuthorizationState::AuthorizedExpired(unauth) => {
                    ui.label("Authorized but expired");
                    if ui.button("Refresh").clicked() {
                        unauth.refresh(&mut self.auth);
                    }
                    true
                }
                AuthorizationState::Unauthorized(unauth) => {
                    ui.label("Unauthorized");
                    if ui.link("Copy Login Link").clicked() {
                        ctx.copy_text(self.auth.get_auth_url().into());
                        unauth.authorize(&mut self.auth);
                    }
                    false
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
                    }

                    true
                }
                AuthorizationState::Authorizing => {
                    ui.label("Authorizing....");
                    ui.spinner();
                    if ui.link("Copy Login Link").clicked() {
                        ctx.copy_text(self.auth.get_auth_url().into());
                    }
                    false
                }
                AuthorizationState::Refreshing => {
                    ui.label("Refreshing....");
                    ui.spinner();
                    true
                }
            };

            if !is_authorized {
                ui.centered_and_justified(|ui| ui.label("Log in first please"));
                return;
            };

            ui.separator();

            ui.button("Discover Channels");
            ui.button("Find new uploads");
            ui.button("Add new uploads to playlist");

            ui.add(ProgressBar::new(0.2));
        });
    }
}
