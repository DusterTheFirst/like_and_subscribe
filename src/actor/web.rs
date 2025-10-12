use std::net::SocketAddr;

use axum::{
    extract::{Query, Request, State},
    middleware::{self, Next},
    response::{Html, IntoResponse as _},
    routing::method_routing,
};
use axum_extra::routing::RouterExt;
use color_eyre::eyre::Context as _;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::oauth::TokenManager;

pub async fn web_server(
    shutdown: CancellationToken,
    token_manager: TokenManager,
) -> color_eyre::Result<()> {
    let tailscale_auth = middleware::from_fn(|req: Request, next: Next| async {
        // TODO: Verify that these are filtered by tailscale funnel
        if req.headers().contains_key("Tailscale-User-Login") {
            next.run(req).await
        } else {
            axum::http::StatusCode::UNAUTHORIZED.into_response()
        }
    });

    let admin_router = axum::Router::new()
        .route_with_tsr("/auth", {
            #[derive(Deserialize)]
            struct Params {
                code: oauth2::AuthorizationCode,
            }
            method_routing::get(
                async |Query(params): Query<Params>, State(token_manager): State<TokenManager>| {
                    match token_manager.load_new_token(params.code).await {
                        Ok(()) => Html("<!DOCTYPE html><html><head><script>window.close()</script></head><body>Authenticated</body></html>").into_response(),
                        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#?}")).into_response(),
                    }
                },
            )
            .with_state(token_manager)
        })
        .layer(tailscale_auth);

    let router = axum::Router::new()
        .nest("/admin", admin_router)
        .fallback(method_routing::any(|| async {
            axum::http::StatusCode::FORBIDDEN // TODO: IPBAN or other honeypot
        }))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CompressionLayer::new()),
        );

    axum::serve(
        tokio::net::TcpListener::bind("127.0.0.1:8080")
            .await
            .wrap_err("unable to bind to port 8080")?,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown.cancelled().await })
    .await
    .wrap_err("failed to run axum server")
}
