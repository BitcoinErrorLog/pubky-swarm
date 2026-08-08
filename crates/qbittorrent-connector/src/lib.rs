//! Authenticated qBittorrent 5.x `WebUI` connector.

#![forbid(unsafe_code)]

use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use reqwest::header::{COOKIE, SET_COOKIE};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use url::{Host, Url};

/// qBittorrent connector failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Base URL is unsafe or malformed.
    #[error("invalid qBittorrent URL: {0}")]
    InvalidBaseUrl(String),
    /// Authentication has not completed.
    #[error("qBittorrent connector is not authenticated")]
    NotAuthenticated,
    /// qBittorrent rejected credentials.
    #[error("qBittorrent authentication failed")]
    AuthenticationFailed,
    /// HTTP operation failed.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// qBittorrent returned an unsuccessful response.
    #[error("qBittorrent API returned {status}: {message}")]
    Api {
        /// HTTP status.
        status: StatusCode,
        /// Response body.
        message: String,
    },
    /// Authentication state lock was poisoned.
    #[error("qBittorrent session lock poisoned")]
    LockPoisoned,
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Torrent summary returned by qBittorrent `WebUI` API 5.x.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TorrentInfo {
    /// v1 or v2 infohash used by qBittorrent.
    pub hash: String,
    /// Display name.
    pub name: String,
    /// Download completion in `0.0..=1.0`.
    #[serde(default)]
    pub progress: f64,
    /// Current bytes per second.
    #[serde(default)]
    pub dlspeed: i64,
    /// Current upload bytes per second.
    #[serde(default)]
    pub upspeed: i64,
    /// Connected seeds.
    #[serde(default)]
    pub num_seeds: i64,
    /// Connected leechers.
    #[serde(default)]
    pub num_leechs: i64,
    /// qBittorrent state identifier.
    #[serde(default)]
    pub state: String,
    /// Comma-separated local tags.
    #[serde(default)]
    pub tags: String,
    /// Local category.
    #[serde(default)]
    pub category: String,
    /// Save path.
    #[serde(default)]
    pub save_path: String,
    /// Full payload path. For multi-file torrents this is normally the
    /// torrent root directory; for single-file torrents it is the file.
    #[serde(default)]
    pub content_path: String,
    /// Total bytes.
    #[serde(default)]
    pub size: i64,
    /// Share ratio.
    #[serde(default)]
    pub ratio: f64,
    /// Estimated seconds remaining.
    #[serde(default)]
    pub eta: i64,
}

/// qBittorrent `WebUI` connector with in-memory SID state.
pub struct QbittorrentClient {
    base_url: Url,
    client: Client,
    sid: Mutex<Option<String>>,
}

impl std::fmt::Debug for QbittorrentClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QbittorrentClient")
            .field("base_url", &self.base_url)
            .field(
                "authenticated",
                &self.sid.lock().is_ok_and(|sid| sid.is_some()),
            )
            .finish_non_exhaustive()
    }
}

impl QbittorrentClient {
    /// Connect only to a loopback qBittorrent `WebUI` endpoint.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback, credential-bearing, or malformed URLs.
    pub fn local(base_url: &str) -> Result<Self> {
        Self::build(base_url, false)
    }

    /// Connect to an explicitly user-approved remote `WebUI` endpoint.
    ///
    /// This permits remote hosts but still rejects URL credentials, query
    /// strings, fragments, and non-HTTP schemes.
    ///
    /// # Errors
    ///
    /// Rejects unsafe or malformed URLs.
    pub fn approved_remote(base_url: &str) -> Result<Self> {
        Self::build(base_url, true)
    }

    fn build(base_url: &str, allow_remote: bool) -> Result<Self> {
        let mut url =
            Url::parse(base_url).map_err(|error| Error::InvalidBaseUrl(error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::InvalidBaseUrl(
                "expected credential-free HTTP(S) origin".to_owned(),
            ));
        }
        if !allow_remote && !is_loopback(&url) {
            return Err(Error::InvalidBaseUrl(
                "local connector requires localhost or loopback IP".to_owned(),
            ));
        }
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        Ok(Self {
            base_url: url,
            client: Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()?,
            sid: Mutex::new(None),
        })
    }

    /// Authenticate and retain the `WebUI` SID in memory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AuthenticationFailed`] for rejected credentials.
    pub async fn login(&self, username: &str, password: &str) -> Result<()> {
        let response = self
            .client
            .post(self.endpoint("api/v2/auth/login")?)
            .form(&[("username", username), ("password", password)])
            .send()
            .await?;
        let cookie = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(parse_sid);
        if !response.status().is_success() || response.text().await? != "Ok." {
            return Err(Error::AuthenticationFailed);
        }
        let cookie = cookie.ok_or(Error::AuthenticationFailed)?;
        *self.sid()? = Some(cookie);
        Ok(())
    }

    /// End the `WebUI` session and clear SID state.
    ///
    /// # Errors
    ///
    /// Returns HTTP/API/lock failures.
    pub async fn logout(&self) -> Result<()> {
        let response = self
            .authorized(self.client.post(self.endpoint("api/v2/auth/logout")?))?
            .send()
            .await?;
        ensure_success(response).await?;
        *self.sid()? = None;
        Ok(())
    }

    /// Return qBittorrent application version.
    ///
    /// # Errors
    ///
    /// Returns HTTP or authentication failures.
    pub async fn version(&self) -> Result<String> {
        let response = self
            .authorized(self.client.get(self.endpoint("api/v2/app/version")?))?
            .send()
            .await?;
        let response = ensure_success(response).await?;
        Ok(response.text().await?)
    }

    /// Add a magnet with optional save path, local category, and local tags.
    ///
    /// # Errors
    ///
    /// Returns authentication, HTTP, or API errors.
    pub async fn add_magnet(
        &self,
        magnet: &str,
        save_path: Option<&str>,
        category: Option<&str>,
        tags: &[String],
    ) -> Result<()> {
        if !magnet.starts_with("magnet:?") {
            return Err(Error::InvalidBaseUrl("invalid magnet URI".to_owned()));
        }
        let mut form = reqwest::multipart::Form::new().text("urls", magnet.to_owned());
        if let Some(path) = save_path {
            form = form.text("savepath", path.to_owned());
        }
        if let Some(category) = category {
            form = form.text("category", category.to_owned());
        }
        if !tags.is_empty() {
            form = form.text("tags", tags.join(","));
        }
        let response = self
            .authorized(
                self.client
                    .post(self.endpoint("api/v2/torrents/add")?)
                    .multipart(form),
            )?
            .send()
            .await?;
        ensure_success(response).await?;
        Ok(())
    }

    /// List torrents and their local qBittorrent metadata.
    ///
    /// # Errors
    ///
    /// Returns authentication, HTTP, API, or JSON errors.
    pub async fn torrents(&self) -> Result<Vec<TorrentInfo>> {
        let response = self
            .authorized(self.client.get(self.endpoint("api/v2/torrents/info")?))?
            .send()
            .await?;
        let response = ensure_success(response).await?;
        Ok(response.json().await?)
    }

    /// Export canonical `.torrent` bytes for one qBittorrent hash.
    ///
    /// # Errors
    ///
    /// Returns authentication, HTTP, API, or size-limit errors.
    pub async fn export_torrent(&self, hash: &str, maximum_bytes: usize) -> Result<Vec<u8>> {
        let mut response = self
            .authorized(
                self.client
                    .get(self.endpoint("api/v2/torrents/export")?)
                    .query(&[("hash", hash)]),
            )?
            .send()
            .await?;
        if !response.status().is_success() {
            return ensure_success(response).await.map(|_| Vec::new());
        }
        if response
            .content_length()
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(Error::Api {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                message: format!("export exceeds {maximum_bytes} byte limit"),
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > maximum_bytes {
                return Err(Error::Api {
                    status: StatusCode::PAYLOAD_TOO_LARGE,
                    message: format!("export exceeds {maximum_bytes} byte limit"),
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base_url
            .join(path)
            .map_err(|error| Error::InvalidBaseUrl(error.to_string()))
    }

    fn authorized(&self, request: RequestBuilder) -> Result<RequestBuilder> {
        let sid = self.sid()?.clone().ok_or(Error::NotAuthenticated)?;
        Ok(request.header(COOKIE, format!("SID={sid}")))
    }

    fn sid(&self) -> Result<MutexGuard<'_, Option<String>>> {
        self.sid.lock().map_err(|_| Error::LockPoisoned)
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    }
}

fn parse_sid(cookie: &str) -> Option<String> {
    cookie
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("SID=").map(str::to_owned))
        .filter(|value| !value.is_empty())
}

async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let message = response.text().await.unwrap_or_default();
    Err(Error::Api { status, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_url_policy_and_sid_parsing() {
        assert!(QbittorrentClient::local("http://127.0.0.1:8080").is_ok());
        assert!(QbittorrentClient::local("http://localhost:8080/base").is_ok());
        assert!(QbittorrentClient::local("http://192.168.1.5:8080").is_err());
        assert!(QbittorrentClient::local("http://user:pass@localhost:8080").is_err());
        assert_eq!(
            parse_sid("SID=abcdef; HttpOnly; path=/"),
            Some("abcdef".to_owned())
        );
    }

    #[test]
    fn torrent_info_tolerates_partial_api_payload() {
        let value = r#"[{"hash":"abc","name":"Ubuntu"}]"#;
        let torrents: Vec<TorrentInfo> = serde_json::from_str(value).unwrap();
        assert_eq!(torrents.len(), 1);
        assert_eq!(torrents[0].name, "Ubuntu");
        assert_eq!(torrents[0].dlspeed, 0);
    }
}
