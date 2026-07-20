use reqwest::blocking::Client;
use std::sync::OnceLock;
use std::time::Duration;

static SOURCE_CLIENT: OnceLock<Client> = OnceLock::new();

/// Return a cheap clone of the process-wide source client. Reqwest clients
/// share their connection pool when cloned, so chromosome jobs can reuse TLS
/// connections instead of rebuilding a client for every range or retry.
pub fn source() -> Result<Client, String> {
    if let Some(client) = SOURCE_CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("AnnoCAT/0.1 local source transport")
        .build()
        .map_err(|error| format!("cannot create source HTTP client: {error}"))?;
    let _ = SOURCE_CLIENT.set(client);
    Ok(SOURCE_CLIENT
        .get()
        .expect("source client was just initialized")
        .clone())
}
