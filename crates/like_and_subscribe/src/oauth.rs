use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ApplicationSecret {
    pub client_id: String,
    pub project_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_secret: String,
    pub redirect_uris: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApplicationSecretFile {
    pub web: ApplicationSecret,
}
