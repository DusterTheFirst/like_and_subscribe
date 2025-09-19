use std::sync::Arc;

use color_eyre::eyre::{Context, ContextCompat};
use jiff::{SignedDuration, Timestamp};
use mail_send::mail_builder::MessageBuilder;
use oauth2::{
    AccessToken, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EmptyExtraTokenFields, EndpointNotSet, EndpointSet, RedirectUrl, RevocationUrl,
    StandardTokenResponse, TokenResponse, TokenUrl,
    basic::{BasicClient, BasicTokenType},
};
use sea_orm::{DatabaseConnection, DbErr};
use tokio::sync::{Mutex, Notify, mpsc};

use crate::database::{Authentication, OAuth};

#[derive(Clone)]
pub struct TokenManager {
    inner: Arc<TokenManagerInner>,
}
impl TokenManager {
    pub async fn init(
        database: DatabaseConnection,
        client_id: ClientId,
        client_secret: ClientSecret,
        hostname: String,
        mail_send: mpsc::Sender<MessageBuilder<'static>>,
    ) -> Result<Self, DbErr> {
        let oauth_client = BasicClient::new(client_id)
            .set_client_secret(client_secret)
            .set_auth_uri(
                AuthUrl::new("https://accounts.google.com/o/oauth2/auth".to_string()).unwrap(),
            )
            .set_token_uri(
                TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap(),
            )
            .set_revocation_url(
                RevocationUrl::new("https://oauth2.googleapis.com/revoke".to_string()).unwrap(),
            )
            .set_redirect_uri(RedirectUrl::new(format!("https://{hostname}/admin/auth")).unwrap());

        let reqwest_client = reqwest::Client::builder()
            // Following redirects opens the client up to SSRF vulnerabilities.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        Ok(Self {
            inner: Arc::new(TokenManagerInner {
                oauth_client,
                reqwest_client,
                mail_send,
                current_token: Mutex::new(match OAuth::get_token(&database).await? {
                    Some(t) => TokenStatus::Existing(t),
                    None => TokenStatus::Missing { alerted: false },
                }),
                notify: Notify::new(),
                database,
            }),
        })
    }

    pub async fn load_new_token(&self, code: AuthorizationCode) -> color_eyre::Result<()> {
        let token_response = self
            .inner
            .oauth_client
            .exchange_code(code)
            .request_async(&self.inner.reqwest_client)
            .await
            .wrap_err("unable to exchange code")?;

        let authentication = Authentication::from_token_response(token_response)?;

        *self.inner.current_token.lock().await = TokenStatus::Existing(authentication.clone());
        self.inner.notify.notify_waiters();
        OAuth::save_token(&self.inner.database, authentication)
            .await
            .wrap_err("unable to save new access token into the database")?;

        Ok(())
    }

    pub async fn wait_for_token(&self) -> Result<AccessToken, DbErr> {
        loop {
            let mut token = self.inner.current_token.lock().await;

            match &mut *token {
                TokenStatus::Existing(authentication) => {
                    if Timestamp::now().duration_until(dbg!(authentication.expires_at))
                        >= SignedDuration::ZERO
                    {
                        return Ok(authentication.access_token.clone());
                    }

                    let refresh_result = self
                        .inner
                        .oauth_client
                        .exchange_refresh_token(&authentication.refresh_token)
                        // Request refresh token
                        .add_extra_param("access_type", "offline")
                        .request_async(&self.inner.reqwest_client)
                        .await;

                    match refresh_result {
                        Ok(token_response) => {
                            let authentication =
                                Authentication::from_token_response(token_response);

                            match authentication {
                                Ok(authentication) => {
                                    let access_token = authentication.access_token.clone();
                                    OAuth::save_token(&self.inner.database, authentication).await?;

                                    return Ok(access_token);
                                }
                                Err(error) => {
                                    tracing::error!(%error, "failed to handle token response");
                                    OAuth::remove_token(&self.inner.database).await?;
                                    self.send_email().await;
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, "failed to refresh access token");
                            OAuth::remove_token(&self.inner.database).await?;
                            self.send_email().await;
                        }
                    }
                }
                TokenStatus::Missing { alerted: true } => {}
                TokenStatus::Missing {
                    alerted: alerted @ false,
                } => {
                    self.send_email().await;
                    *alerted = true;
                }
            };

            // Wait for token to be loaded
            drop(token);
            tracing::debug!("waiting for new token to be obtained");
            self.inner.notify.notified().await;
        }
    }

    // TODO: explain the reason for the re-auth
    async fn send_email(&self) {
        tracing::info!("Queuing email");
        let (authorize_url, _) = self
            .inner
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

        let message = MessageBuilder::new()
            .subject("Re-authenticate with google to continue")
            .html_body(format!(r##"<a href="{0}">{0}</a>"##, authorize_url));

        self.inner.mail_send.send(message).await.unwrap();
    }
}

impl Authentication {
    pub fn from_token_response(
        token_response: StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>,
    ) -> color_eyre::Result<Self> {
        Ok(Authentication {
            access_token: token_response.access_token().clone(),
            refresh_token: token_response
                .refresh_token()
                .wrap_err("no refresh token was provided")?
                .clone(),
            expires_at: Timestamp::now()
                + token_response
                    .expires_in()
                    .wrap_err("no expiration was provided")?,
        })
    }
}

struct TokenManagerInner {
    oauth_client:
        BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet, EndpointSet>,
    mail_send: mpsc::Sender<MessageBuilder<'static>>,

    reqwest_client: reqwest::Client,
    database: DatabaseConnection,

    current_token: Mutex<TokenStatus>,
    notify: Notify,
}

enum TokenStatus {
    Missing { alerted: bool },
    Existing(Authentication),
}
