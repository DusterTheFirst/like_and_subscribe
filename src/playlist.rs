use color_eyre::eyre::Context;
use google_youtube3::{
    YouTube,
    api::{PlaylistItem, PlaylistItemSnippet, ResourceId},
};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use jiff::Unit;
use tokio::sync::mpsc::Receiver;
use tracing::{Instrument, debug, trace};

use crate::feed::Feed;

pub async fn youtube_playlist_modifier(
    youtube: YouTube<HttpsConnector<HttpConnector>>,
    playlist_id: String,
    mut reciever: Receiver<(tracing::Span, Feed)>,
) -> color_eyre::Result<()> {
    while let Some((span, Feed { ref entry, .. })) = reciever.recv().await {
        async {
            debug!("validating new feed item");

            let video_age_minutes = (entry.updated - entry.published)
                .total((Unit::Minute, entry.updated))
                .unwrap();

            span.record("video_age_minutes", video_age_minutes);

            match video_age_minutes {
                ..=1.0 => {
                    // TODO: duplicate detection
                    // TODO: shorts detection
                    debug!("inserting new video");
                    span.record("inserted", true);

                    youtube
                        .playlist_items()
                        .insert(PlaylistItem {
                            snippet: Some(PlaylistItemSnippet {
                                playlist_id: Some(playlist_id.clone()),
                                resource_id: Some(ResourceId {
                                    kind: Some("youtube#video".into()),
                                    video_id: Some(entry.video_id.clone()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .doit()
                        .await
                        .map(|_| ())
                        .wrap_err("failed to insert playlist item")
                }
                _ => {
                    trace!("ignoring updated old video");
                    span.record("inserted", false);

                    Ok(())
                }
            }
        }
        .instrument(span.clone())
        .await?;
    }

    Ok(())
}
