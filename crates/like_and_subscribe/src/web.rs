use std::{
    net::SocketAddr,
    sync::Arc,
};

use axum::routing::method_routing;
use axum_extra::routing::RouterExt;
use color_eyre::eyre::Context as _;
use sea_orm::DatabaseConnection;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

mod pubsub;

pub async fn web_server(
    shutdown: CancellationToken,
    database: DatabaseConnection,
    video_queue_notify: Arc<Notify>,
) -> color_eyre::Result<()> {
    // let tailscale_auth = middleware::from_fn(|req: Request, next: Next| async {
    //     // TODO: Verify that these are filtered by tailscale funnel
    //     if req.headers().contains_key("Tailscale-User-Login") {
    //         next.run(req).await
    //     } else {
    //         axum::http::StatusCode::UNAUTHORIZED.into_response()
    //     }
    // });

    // let admin_router = axum::Router::new()
    //     .nest_service("/admin", method_routing::get(|| async { "TODO:" }))
    //     .layer(tailscale_auth);

    let pubsub_router = axum::Router::new().route_with_tsr(
        "/pubsub",
        method_routing::get(pubsub::pubsub_subscription_validation)
            .with_state(database.clone())
            .post(pubsub::pubsub_new_upload)
            .with_state((database, video_queue_notify)),
    );

    let router = axum::Router::new()
        // .merge(admin_router)
        .merge(pubsub_router)
        .fallback(method_routing::any(|| async {
            axum::http::StatusCode::PAYMENT_REQUIRED // TODO: IPBAN or other honeypot
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
