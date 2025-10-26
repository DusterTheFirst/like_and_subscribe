use std::{sync::Arc, thread::JoinHandle};

use eframe::egui::Context;
use jiff::Timestamp;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    RedirectUrl, RefreshToken, RevocationUrl, TokenResponse, TokenUrl, basic::BasicClient, ureq,
    url::Url,
};
use tiny_http::{Header, Response};
use tracing::info;

use crate::database::Database;

pub struct OAuthManager {
    oauth_client:
        Arc<BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet, EndpointSet>>,
    database: Database,

    context: Context,

    state: OAuthState,
}

#[derive(Debug)]
enum OAuthState {
    Uninitialized {
        refresh_token: Option<oauth2::RefreshToken>,
    },
    Authorizing {
        handle: JoinHandle<(oauth2::RefreshToken, oauth2::AccessToken, Timestamp)>,
    },
    Refreshing {
        refresh_token: oauth2::RefreshToken,
        handle: JoinHandle<(oauth2::AccessToken, Timestamp)>,
    },
    Authorized {
        refresh_token: oauth2::RefreshToken,
        access_token: oauth2::AccessToken,
        expires_at: Timestamp,
    },
}

impl Default for OAuthState {
    fn default() -> Self {
        OAuthState::Uninitialized {
            refresh_token: None,
        }
    }
}

impl OAuthManager {
    pub fn get_state(&mut self) -> AuthorizationState {
        fn handle_authorized(
            refresh_token: oauth2::RefreshToken,
            access_token: oauth2::AccessToken,
            expires_at: Timestamp,
            ctx: &Context,
        ) -> AuthorizationState {
            let validity_period = Timestamp::now().duration_until(expires_at);

            if validity_period.is_negative() {
                return AuthorizationState::AuthorizedExpired(AuthorizedExpired { refresh_token });
            }

            ctx.request_repaint_after_secs(validity_period.as_secs_f32());

            AuthorizationState::Authorized(Authorized {
                refresh_token,
                access_token,
                expires_at,
            })
        }

        match std::mem::take(&mut self.state) {
            state @ OAuthState::Uninitialized {
                refresh_token: None,
            } => {
                self.state = state;
                AuthorizationState::Unauthorized(Unauthorized {})
            }
            OAuthState::Uninitialized {
                refresh_token: Some(refresh_token),
            } => {
                self.refresh(refresh_token.clone());
                AuthorizationState::Refreshing
            }
            OAuthState::Authorizing { handle } => {
                if handle.is_finished() {
                    let (refresh_token, access_token, expires_at) = handle.join().unwrap();

                    self.state = OAuthState::Authorized {
                        refresh_token: refresh_token.clone(),
                        access_token: access_token.clone(),
                        expires_at,
                    };
                    handle_authorized(refresh_token, access_token, expires_at, &self.context)
                } else {
                    self.state = OAuthState::Authorizing { handle };
                    AuthorizationState::Authorizing
                }
            }
            OAuthState::Refreshing {
                refresh_token,
                handle,
            } => {
                if handle.is_finished() {
                    let (access_token, expires_at) = handle.join().unwrap();

                    self.state = OAuthState::Authorized {
                        refresh_token: refresh_token.clone(),
                        access_token: access_token.clone(),
                        expires_at,
                    };
                    handle_authorized(refresh_token, access_token, expires_at, &self.context)
                } else {
                    self.state = OAuthState::Refreshing {
                        refresh_token,
                        handle,
                    };
                    AuthorizationState::Refreshing
                }
            }
            OAuthState::Authorized {
                refresh_token,
                access_token,
                expires_at,
            } => {
                self.state = OAuthState::Authorized {
                    refresh_token: refresh_token.clone(),
                    access_token: access_token.clone(),
                    expires_at,
                };
                handle_authorized(refresh_token, access_token, expires_at, &self.context)
            }
        }
    }
}

pub enum AuthorizationState {
    Unauthorized(Unauthorized),
    Authorized(Authorized),
    AuthorizedExpired(AuthorizedExpired),
    Refreshing,
    Authorizing,
}

pub trait Refresh: Sized {
    fn refresh_token(self) -> oauth2::RefreshToken;

    fn refresh(self, auth_mgr: &mut OAuthManager) {
        auth_mgr.refresh(self.refresh_token());
    }
}

pub trait Authorize: Sized {
    fn authorize(self, auth_mgr: &mut OAuthManager) {
        auth_mgr.authorize();
    }
}

pub struct Unauthorized {}
impl Authorize for Unauthorized {}

pub struct Authorized {
    pub access_token: oauth2::AccessToken,
    pub refresh_token: oauth2::RefreshToken,
    pub expires_at: Timestamp,
}
impl Refresh for Authorized {
    fn refresh_token(self) -> oauth2::RefreshToken {
        self.refresh_token
    }
}

pub struct AuthorizedExpired {
    pub refresh_token: oauth2::RefreshToken,
}
impl Refresh for AuthorizedExpired {
    fn refresh_token(self) -> oauth2::RefreshToken {
        self.refresh_token
    }
}

impl OAuthManager {
    pub fn new(
        client_id: ClientId,
        client_secret: ClientSecret,
        database: Database,
        context: Context,
    ) -> Self {
        Self {
            state: OAuthState::Uninitialized {
                refresh_token: database.oauth().get(),
            },
            database,
            context,
            oauth_client: Arc::new(
                BasicClient::new(client_id)
                    .set_client_secret(client_secret)
                    .set_auth_uri(
                        AuthUrl::new("https://accounts.google.com/o/oauth2/auth".to_string())
                            .unwrap(),
                    )
                    .set_token_uri(
                        TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap(),
                    )
                    .set_revocation_url(
                        RevocationUrl::new("https://oauth2.googleapis.com/revoke".to_string())
                            .unwrap(),
                    )
                    .set_redirect_uri(
                        RedirectUrl::new("http://localhost:8081".to_string()).unwrap(),
                    ),
            ),
        }
    }

    pub fn get_auth_url(&self) -> Url {
        let (authorize_url, _) = self
            .oauth_client
            .authorize_url(|| CsrfToken::new("TODO:FIXME:?".to_string()))
            .add_scope(oauth2::Scope::new(
                "https://www.googleapis.com/auth/youtube.readonly".to_string(),
            ))
            .add_scope(oauth2::Scope::new(
                "https://www.googleapis.com/auth/youtube".to_string(),
            ))
            // The following 2 parameters ask for a refresh token
            .add_extra_param("access_type", "offline")
            .add_extra_param("prompt", "consent")
            .url();

        authorize_url
    }

    fn authorize(&mut self) {
        info!("Authorizing");

        let handle = std::thread::Builder::new()
            .name("oauth".to_owned())
            .spawn({
                let oauth_client = self.oauth_client.clone();
                let database = self.database.clone();
                let ctx = self.context.clone();

                move || {
                    let base_url = Url::parse("http://localhost:8081").unwrap();
                    let server = tiny_http::Server::http("localhost:8081").unwrap();

                    let request = server.incoming_requests().next().unwrap();

                    let url = base_url.join(request.url()).unwrap();

                    let (_, code) = url
                        .query_pairs()
                        .find(|(key, _)| key.eq_ignore_ascii_case("code"))
                        .expect("code url param should exist");

                    let code = AuthorizationCode::new(code.into_owned());

                    let token_response = oauth_client
                        .exchange_code(code)
                        .request(&oauth2::ureq::agent())
                        .unwrap();

                    let expires_at = Timestamp::now() + token_response.expires_in().expect("expiration should be provided");
                    let refresh_token = token_response.refresh_token().expect("offline authorization should always provide a refresh token");

                    database.oauth().set(refresh_token.clone());

                    const HTML: &str = "<!DOCTYPE html><html><body>Authorized, you may close this window</body></html>";

                    let response = Response::from_string(HTML)
                        .with_header(Header::from_bytes(b"Content-Type", b"text/html").unwrap());
                    request.respond(response).unwrap();

                    ctx.request_repaint();
                    (refresh_token.clone(), token_response.access_token().clone(), expires_at)
                }}
            )
            .unwrap();

        self.state = OAuthState::Authorizing { handle };
    }

    fn refresh(&mut self, refresh_token: RefreshToken) {
        let handle = std::thread::Builder::new()
            .name("oauth".to_owned())
            .spawn({
                let oauth_client = self.oauth_client.clone();
                let refresh_token = refresh_token.clone();
                let ctx: Context = self.context.clone();

                move || {
                    info!("Refreshing");
                    let refresh_result = match oauth_client
                        .exchange_refresh_token(&refresh_token)
                        .request(&ureq::agent())
                    {
                        Ok(res) => res,
                        Err(oauth2::RequestTokenError::Request(
                            oauth2::HttpClientError::Reqwest(error),
                        )) => {
                            match *error {
                                ureq::Error::Status(code, response) => panic!(
                                    "{code}: {:?}",
                                    response.into_json::<serde_json::Value>()
                                ),
                                _ => panic!("{error}"),
                            };
                        }
                        Err(error) => {
                            panic!("{error}")
                        }
                    };

                    let expires_at = Timestamp::now()
                        + refresh_result
                            .expires_in()
                            .expect("expiration should be provided");

                    info!("refreshed");

                    ctx.request_repaint();
                    (refresh_result.access_token().clone(), expires_at)
                }
            })
            .unwrap();

        self.state = OAuthState::Refreshing {
            refresh_token,
            handle,
        };
    }
}
