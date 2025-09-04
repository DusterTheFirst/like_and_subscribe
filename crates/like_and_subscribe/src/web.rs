use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, Response},
    middleware::{self, Next},
    response::IntoResponse,
    routing::method_routing,
};
use axum_extra::{TypedHeader, routing::RouterExt};
use color_eyre::eyre::Context as _;
use tower::ServiceBuilder;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

pub async fn web_server(
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> color_eyre::Result<()> {
    let admin_files =
        std::env::var_os("ADMIN_PANEL_FILES").expect("ADMIN_PANEL_FILES should be set");
    let admin_files = Path::new(&admin_files);

    let admin_router = method_routing::get_service(ServeDir::new(admin_files).fallback(
        ServeFile::new(PathBuf::from_iter([admin_files, Path::new("index.html")])),
    ))
    .route_layer(middleware::from_fn(|req: Request, next: Next| async {
        if req.headers().contains_key("Tailscale-User-Login") {
            next.run(req).await
        } else {
            axum::http::StatusCode::UNAUTHORIZED.into_response()
        }
    }));

    let router = axum::Router::new()
        .route_with_tsr("/pubsub", {
            method_routing::get(crate::pubsub::pubsub_subscription)
                .with_state(subscriptions.clone())
                .post(crate::pubsub::pubsub_new_upload)
                .with_state(new_video_channel)
        })
        .nest_service("/admin", admin_router)
        .fallback(method_routing::any(|| async {
            axum::http::StatusCode::PAYMENT_REQUIRED // TODO: IPBAN or other honeypot
        }))
        .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()));

    axum::serve(
        tokio::net::TcpListener::bind("localhost:8080")
            .await
            .wrap_err("unable to bind to port 8080")?,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.recv().await;
    })
    .await
    .wrap_err("failed to run axum server")
}
