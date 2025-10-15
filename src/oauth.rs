use std::{sync::Arc, thread::JoinHandle};

use eframe::egui::Context;
use jiff::Timestamp;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
    EndpointNotSet, EndpointSet, RedirectUrl, RefreshToken, RevocationUrl, StandardTokenResponse,
    TokenResponse, TokenUrl,
    basic::{BasicClient, BasicTokenType},
    ureq,
    url::Url,
};
use tiny_http::{Header, Response};
use tracing::info;

use crate::database::Database;

#[derive(Debug, Clone)]
pub struct Authentication {
    pub access_token: oauth2::AccessToken,
    pub refresh_token: Option<oauth2::RefreshToken>,
    pub expires_at: Timestamp,
}

impl Authentication {
    pub fn from_token_response(
        token_response: StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>,
    ) -> Self {
        Authentication {
            access_token: token_response.access_token().clone(),
            refresh_token: token_response.refresh_token().cloned(),
            expires_at: Timestamp::now()
                + token_response
                    .expires_in()
                    .expect("expiration should be provided"),
        }
    }
}

pub struct AuthenticationManager {
    oauth_client:
        Arc<BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet, EndpointSet>>,
    authentication: Option<AuthenticationInternalState>,
    database: Database,
}

enum AuthenticationInternalState {
    Working {
        refresh: bool,
        handle: JoinHandle<Authentication>,
    },
    Filled(Authentication),
}

impl AuthenticationManager {
    pub fn get_state(&mut self, ctx: &Context) -> AuthenticationState {
        fn handle_authentication(
            authentication: Authentication,
            ctx: &Context,
        ) -> AuthenticationState {
            match authentication {
                Authentication {
                    access_token,
                    refresh_token: None,
                    expires_at,
                } => {
                    let validity_period = Timestamp::now().duration_until(expires_at);

                    if validity_period.is_negative() {
                        return AuthenticationState::Unauthenticated(
                            UnauthenticatedWithoutRefresh {},
                        );
                    }

                    ctx.request_repaint_after_secs(validity_period.as_secs_f32());

                    AuthenticationState::Authenticated(AuthenticatedWithoutRefresh {
                        access_token,
                        expires_at,
                    })
                }
                Authentication {
                    access_token,
                    refresh_token: Some(refresh_token),
                    expires_at,
                } => {
                    let validity_period = Timestamp::now().duration_until(expires_at);

                    if validity_period.is_negative() {
                        return AuthenticationState::UnauthenticatedRefresh(
                            UnauthenticatedWithRefresh { refresh_token },
                        );
                    }

                    ctx.request_repaint_after_secs(validity_period.as_secs_f32());

                    AuthenticationState::AuthenticatedRefresh(AuthenticatedWithRefresh {
                        access_token,
                        expires_at,
                        refresh_token,
                    })
                }
            }
        }

        match self.authentication.take() {
            Some(AuthenticationInternalState::Working { refresh, handle }) => {
                if handle.is_finished() {
                    let auth = handle.join().unwrap();
                    self.authentication = Some(AuthenticationInternalState::Filled(auth.clone()));
                    handle_authentication(auth, ctx)
                } else {
                    self.authentication =
                        Some(AuthenticationInternalState::Working { refresh, handle });

                    if refresh {
                        AuthenticationState::Refreshing
                    } else {
                        AuthenticationState::Authenticating
                    }
                }
            }
            Some(AuthenticationInternalState::Filled(auth)) => {
                self.authentication = Some(AuthenticationInternalState::Filled(auth.clone()));
                handle_authentication(auth, ctx)
            }
            None => AuthenticationState::Unauthenticated(UnauthenticatedWithoutRefresh {}),
        }
    }
}

pub enum AuthenticationState {
    UnauthenticatedRefresh(UnauthenticatedWithRefresh),
    Unauthenticated(UnauthenticatedWithoutRefresh),
    AuthenticatedRefresh(AuthenticatedWithRefresh),
    Authenticated(AuthenticatedWithoutRefresh),
    Authenticating,
    Refreshing,
}

pub trait Refresh: Sized {
    fn refresh_token(self) -> oauth2::RefreshToken;

    fn refresh(self, auth_mgr: &mut AuthenticationManager, ctx: Context) {
        auth_mgr.refresh(self.refresh_token(), ctx);
    }
}

pub trait Authenticate: Sized {
    fn authenticate(self, auth_mgr: &mut AuthenticationManager, ctx: Context) {
        auth_mgr.authenticate(ctx);
    }
}

pub struct UnauthenticatedWithRefresh {
    pub refresh_token: oauth2::RefreshToken,
}
impl Refresh for UnauthenticatedWithRefresh {
    fn refresh_token(self) -> oauth2::RefreshToken {
        self.refresh_token
    }
}

pub struct UnauthenticatedWithoutRefresh {}
impl Authenticate for UnauthenticatedWithoutRefresh {}

pub struct AuthenticatedWithRefresh {
    pub access_token: oauth2::AccessToken,
    pub refresh_token: oauth2::RefreshToken,
    pub expires_at: Timestamp,
}
impl Refresh for AuthenticatedWithRefresh {
    fn refresh_token(self) -> oauth2::RefreshToken {
        self.refresh_token
    }
}
pub struct AuthenticatedWithoutRefresh {
    pub access_token: oauth2::AccessToken,
    pub expires_at: Timestamp,
}
impl Authenticate for AuthenticatedWithoutRefresh {}

impl AuthenticationManager {
    pub fn new(client_id: ClientId, client_secret: ClientSecret, database: Database) -> Self {
        Self {
            authentication: database
                .oauth()
                .get()
                .map(AuthenticationInternalState::Filled),
            database,
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

    fn authenticate(&mut self, ctx: Context) {
        info!("Authenticating");
        let oauth_client = self.oauth_client.clone();
        let database = self.database.clone();

        let handle = std::thread::Builder::new()
            .name("oauth".to_owned())
            .spawn(move || {
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

                let authentication = Authentication::from_token_response(token_response);

                database.oauth().set(authentication.clone());

                const HTML: &str = "<!DOCTYPE html><html><body>Authenticated, you may close this window</body></html>";

                let response = Response::from_string(HTML)
                    .with_header(Header::from_bytes(b"Content-Type", b"text/html").unwrap());
                request.respond(response).unwrap();

                ctx.request_repaint();
                authentication
            })
            .unwrap();

        self.authentication = Some(AuthenticationInternalState::Working {
            handle,
            refresh: false,
        });
    }

    fn refresh(&mut self, refresh_token: RefreshToken, ctx: Context) {
        info!("Refreshing");
        let oauth_client = self.oauth_client.clone();
        let database = self.database.clone();

        let handle = std::thread::Builder::new()
            .name("oauth".to_owned())
            .spawn(move || {
                let refresh_result = oauth_client
                    .exchange_refresh_token(&refresh_token)
                    // Request refresh token
                    .add_extra_param("access_type", "offline")
                    .request(&ureq::agent())
                    .unwrap();

                let authentication = Authentication::from_token_response(refresh_result);

                database.oauth().set(authentication.clone());

                ctx.request_repaint();
                authentication
            })
            .unwrap();

        self.authentication = Some(AuthenticationInternalState::Working {
            handle,
            refresh: true,
        });
    }
}
