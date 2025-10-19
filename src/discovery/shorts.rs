#[cfg(test)]
mod test {

    use super::check_redirect;

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

    #[test]
    fn test_check_redirect() {
        for &(video_id, expected_is_short) in VIDEO_IDS {
            let is_short = check_redirect(video_id)
                .unwrap_or_else(|err| panic!("failed to check video {video_id}: {err:?}"));

            assert_eq!(is_short, expected_is_short)
        }
    }
}

#[derive(Debug)]
pub enum CheckRedirectError {
    BadRequest,
    BadResponse,
    NonWatchRedirect(String),
}

pub fn check_redirect(video_id: &str) -> Result<bool, CheckRedirectError> {
    let result = ureq::AgentBuilder::new()
        .redirects(0)
        .build()
        .head(&format!("https://www.youtube.com/shorts/{}", video_id))
        .call();

    let response = match result {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "failed to request shorts url");
            return Err(CheckRedirectError::BadRequest);
        }
    };

    if response.status() == 200 {
        Ok(true)
    } else if response.status() >= 300 && response.status() < 400 {
        let Some(location) = response.header("Location") else {
            tracing::error!(
                ?response,
                "redirect response did not contain a Location header"
            );
            return Err(CheckRedirectError::BadResponse);
        };

        if location.contains("watch") {
            Ok(false)
        } else {
            Err(CheckRedirectError::NonWatchRedirect(location.to_owned()))
        }
    } else {
        tracing::error!(?response, "redirect response had unexpected status code");
        Err(CheckRedirectError::BadResponse)
    }
}
