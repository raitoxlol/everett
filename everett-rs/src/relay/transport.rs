pub(super) struct Client {
    stored: crate::cloudflare_auth::Credential,
}

impl Client {
    pub fn native(stored: crate::cloudflare_auth::Credential) -> Self {
        Self { stored }
    }

    pub async fn call(
        &mut self,
        name: &str,
        value: serde_json::Value,
    ) -> crate::error::Result<serde_json::Value> {
        let stored = self.stored.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || stored.relay_request(&name, &value))
            .await
            .map_err(|_| super::bad("Cloudflare relay worker failed"))?
    }
}
