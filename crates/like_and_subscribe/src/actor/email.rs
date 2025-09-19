use mail_send::{
    Credentials, SmtpClientBuilder,
    mail_builder::{MessageBuilder, headers::address::Address},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub async fn email_sender(
    shutdown: CancellationToken,
    email_credentials: Credentials<String>,
    mut email_send_rx: mpsc::Receiver<MessageBuilder<'static>>,
) -> Result<(), ()> {
    let mut smtp = SmtpClientBuilder::new("smtp.fastmail.com".to_string(), 465)
        .credentials(email_credentials)
        .connect()
        .await
        .unwrap();

    loop {
        let email = tokio::select! {
            _ = shutdown.cancelled() => break,
            email = email_send_rx.recv() => {email}
        };

        let Some(email) = email else {
            break;
        };

        let email = email
            .from(Address::new_address(Some("Alerts"), "alerts@kohnen.dev"))
            .to(Address::new_address(
                Some("Zachary Kohnen"),
                "me@dusterthefirst.com",
            ));

        // FIXME: do we need to reconnect to the smtp server each time?
        if let Err(error) = smtp.send(email).await {
            tracing::error!(%error, "failed to send email");
        } else {
            tracing::info!("sent alert email");
        }
    }

    _ = smtp.quit().await.inspect_err(
        |error| tracing::error!(%error, "failed to send quit message to the smtp server"),
    );

    tracing::info!("shutting down");

    Ok(())
}
