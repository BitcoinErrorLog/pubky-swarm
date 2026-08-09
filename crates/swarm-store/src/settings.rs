//! Persistent desktop torrent-client settings.

use rusqlite::{OptionalExtension, params};

use crate::{Error, Result, Store};

/// Maximum persisted download or upload throttle, in kibibytes per second.
pub const MAX_LIMIT_KBPS: u32 = 1_000_000;

/// User-configurable torrent client preferences.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSettings {
    /// Default download directory. `None` means the app data `downloads` folder.
    pub download_dir: Option<String>,
    /// Whether the BitTorrent Mainline DHT should be enabled on next engine start.
    pub dht_enabled: bool,
    /// Whether UPnP port forwarding should be enabled on next engine start.
    pub upnp_enabled: bool,
    /// Download throttle in KiB/s. `None` means unlimited.
    pub download_limit_kbps: Option<u32>,
    /// Upload throttle in KiB/s. `None` means unlimited.
    pub upload_limit_kbps: Option<u32>,
    /// Preferred TCP listen port. `None` means auto-select a free port.
    pub listen_port: Option<u16>,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            download_dir: None,
            dht_enabled: true,
            upnp_enabled: true,
            download_limit_kbps: None,
            upload_limit_kbps: None,
            listen_port: None,
        }
    }
}

impl ClientSettings {
    /// Validate path / port / rate bounds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidClientSettings`] when a field is out of range.
    pub fn validate(&self) -> Result<()> {
        if let Some(path) = &self.download_dir {
            let trimmed = path.trim();
            if trimmed.is_empty() || trimmed.len() > 4096 {
                return Err(Error::InvalidClientSettings(
                    "download_dir must be 1..=4096 characters".to_owned(),
                ));
            }
            if trimmed.chars().any(|c| c == '\0' || c.is_control()) {
                return Err(Error::InvalidClientSettings(
                    "download_dir contains invalid characters".to_owned(),
                ));
            }
        }
        if let Some(port) = self.listen_port
            && port == 0
        {
            return Err(Error::InvalidClientSettings(
                "listen_port must be 1..=65535 when set".to_owned(),
            ));
        }
        for (name, value) in [
            ("download_limit_kbps", self.download_limit_kbps),
            ("upload_limit_kbps", self.upload_limit_kbps),
        ] {
            if let Some(kbps) = value
                && kbps > MAX_LIMIT_KBPS
            {
                return Err(Error::InvalidClientSettings(format!(
                    "{name} must be at most {MAX_LIMIT_KBPS}"
                )));
            }
        }
        Ok(())
    }

    /// Normalize empty optional strings and zero rate limits to `None`.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        if let Some(path) = self.download_dir.take() {
            let trimmed = path.trim().to_owned();
            self.download_dir = (!trimmed.is_empty()).then_some(trimmed);
        }
        if self.download_limit_kbps == Some(0) {
            self.download_limit_kbps = None;
        }
        if self.upload_limit_kbps == Some(0) {
            self.upload_limit_kbps = None;
        }
        self
    }
}

impl Store {
    /// Return the persisted client settings, or defaults when unset.
    pub fn client_settings(&self) -> Result<ClientSettings> {
        let connection = self.connection()?;
        let row: Option<(
            Option<String>,
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
        )> = connection
            .query_row(
                "SELECT download_dir, dht_enabled, upnp_enabled,
                        download_limit_kbps, upload_limit_kbps, listen_port
                 FROM client_settings WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((download_dir, dht, upnp, down, up, port)) = row else {
            return Ok(ClientSettings::default());
        };
        Ok(ClientSettings {
            download_dir,
            dht_enabled: dht != 0,
            upnp_enabled: upnp != 0,
            download_limit_kbps: down.map(|value| u32::try_from(value).unwrap_or(0)),
            upload_limit_kbps: up.map(|value| u32::try_from(value).unwrap_or(0)),
            listen_port: port.and_then(|value| u16::try_from(value).ok()),
        })
    }

    /// Persist validated client settings.
    pub fn set_client_settings(&self, settings: &ClientSettings) -> Result<()> {
        let settings = settings.clone().normalized();
        settings.validate()?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO client_settings (
                 id, download_dir, dht_enabled, upnp_enabled,
                 download_limit_kbps, upload_limit_kbps, listen_port
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 download_dir = excluded.download_dir,
                 dht_enabled = excluded.dht_enabled,
                 upnp_enabled = excluded.upnp_enabled,
                 download_limit_kbps = excluded.download_limit_kbps,
                 upload_limit_kbps = excluded.upload_limit_kbps,
                 listen_port = excluded.listen_port",
            params![
                settings.download_dir,
                i64::from(settings.dht_enabled),
                i64::from(settings.upnp_enabled),
                settings.download_limit_kbps.map(i64::from),
                settings.upload_limit_kbps.map(i64::from),
                settings.listen_port.map(i64::from),
            ],
        )?;
        Ok(())
    }
}
