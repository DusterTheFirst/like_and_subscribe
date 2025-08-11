use bstr::ByteSlice as _;
use reqwest::header;

#[cfg(test)]
mod test {
    use std::time::Duration;

    use color_eyre::eyre::{Context as _, eyre};
    use reqwest::redirect::Policy;
    use tower::ServiceBuilder;

    use crate::playlist::shorts::check_redirect;

    const VIDEO_IDS: &[(&str, bool)] = &[
        ("egMU3JBQZO8", true),
        ("FzLIWW3eDlQ", false),
        ("1ycMUB2kSWE", false),
        ("lrZlBPJYH-Y", true),
        ("SqmaeqNsssU", true),
        ("a1geSCiU_fE", true),
        ("aLKN_Rmb39I", false),
        ("8V_W1bIitIc", true),
        ("ZjQPqs1oEOk", false),
    ];

    #[tokio::test]
    async fn test_check_redirect() -> color_eyre::Result<()> {
        let client = reqwest::ClientBuilder::new()
            .https_only(true)
            .connector_layer(
                ServiceBuilder::new()
                    .concurrency_limit(10)
                    .buffer(1024)
                    .rate_limit(5, Duration::from_secs(10)), // TODO: does this mean 5 sets of 10?
            )
            .redirect(Policy::none())
            .build()
            .wrap_err("Unable to setup reqwest client")?;

        for &(video_id, expected_is_short) in VIDEO_IDS {
            let is_short = check_redirect(video_id, &client).await.map_err(|err| {
                eyre!("failed to check video {video_id}").wrap_err(eyre!("{err:?}"))
            })?;

            assert_eq!(is_short, expected_is_short)
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum CheckRedirectError {
    BadRequest,
    BadResponse,
    NonWatchRedirect,
}

pub async fn check_redirect(
    video_id: &str,
    client: &reqwest::Client,
) -> Result<bool, CheckRedirectError> {
    let result = client
        .execute(
            client
                .head(format!("https://www.youtube.com/shorts/{}", video_id))
                .build()
                .unwrap(),
        )
        .await;

    let response = match result {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "failed to request shorts url");
            return Err(CheckRedirectError::BadRequest);
        }
    };
    if response.status().is_success() {
        Ok(true)
    } else if response.status().is_redirection() {
        let Some(location) = response.headers().get(header::LOCATION) else {
            tracing::error!(
                ?response,
                "redirect response did not contain a Location header"
            );
            return Err(CheckRedirectError::BadResponse);
        };

        if location.as_bytes().contains_str("watch") {
            Ok(false)
        } else {
            Err(CheckRedirectError::NonWatchRedirect)
        }
    } else {
        tracing::error!(?response, "redirect response had unexpected status code");
        Err(CheckRedirectError::BadResponse)
    }
}
