//! Pubky 0.10 storage and event adapter for the Swarm namespace.

#![forbid(unsafe_code)]

use bytes::Bytes;
use pubky::{
    EventCursor, EventStreamBuilder, Pubky, PubkyResource, PubkySession, PublicKey, Result,
};
use serde::{Serialize, de::DeserializeOwned};

/// Interoperable Pubky App profile location.
pub const PROFILE_PATH: &str = "/pub/pubky.app/profile.json";
/// Versioned release-record namespace owned by this application.
pub const RELEASES_PATH: &str = "/pub/pubky.swarm/v1/releases/";
/// Versioned issuer-attributed tag-claim namespace.
pub const TAG_CLAIMS_PATH: &str = "/pub/pubky.swarm/v1/tag-claims/";

/// Thin, HTTP-independent application facade over the official Pubky SDK.
#[derive(Debug, Clone)]
pub struct PubkyAdapter {
    sdk: Pubky,
}

impl PubkyAdapter {
    /// Use the main Pubky network.
    ///
    /// # Errors
    ///
    /// Returns the SDK build error if its transports cannot initialize.
    pub fn mainnet() -> Result<Self> {
        Ok(Self { sdk: Pubky::new()? })
    }

    /// Wrap an SDK configured by the caller (including local testnets).
    #[must_use]
    pub const fn with_sdk(sdk: Pubky) -> Self {
        Self { sdk }
    }

    /// Access the official SDK for auth-flow construction and signer actions.
    #[must_use]
    pub const fn sdk(&self) -> &Pubky {
        &self.sdk
    }

    /// Read and deserialize a public JSON resource through Pubky resolution.
    ///
    /// # Errors
    ///
    /// Propagates Pubky resolution, HTTP, status, and JSON errors.
    pub async fn get_public_json<T>(&self, user: &PublicKey, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.sdk.public_storage().get_json((user, path)).await
    }

    /// Read public bytes through Pubky resolution.
    ///
    /// # Errors
    ///
    /// Propagates Pubky resolution, HTTP, status, and body errors.
    pub async fn get_public_bytes(&self, user: &PublicKey, path: &str) -> Result<Bytes> {
        Ok(self
            .sdk
            .public_storage()
            .get((user, path))
            .await?
            .bytes()
            .await?)
    }

    /// List a public directory with pagination controls.
    ///
    /// # Errors
    ///
    /// Propagates path validation, resolution, and HTTP errors.
    pub async fn list_public(
        &self,
        user: &PublicKey,
        directory: &str,
        cursor: Option<&str>,
        limit: u16,
    ) -> Result<Vec<PubkyResource>> {
        let storage = self.sdk.public_storage();
        let mut request = storage.list((user, directory))?.limit(limit);
        if let Some(cursor) = cursor {
            request = request.cursor(cursor);
        }
        match request.send().await {
            Ok(resources) => Ok(resources),
            Err(pubky::Error::Request(pubky::errors::RequestError::Server { status, .. }))
                if status.as_u16() == 404 =>
            {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    /// Write JSON through a capability-scoped authenticated session.
    ///
    /// # Errors
    ///
    /// Propagates path validation, authorization, serialization, and HTTP
    /// errors.
    pub async fn put_json<T>(&self, session: &PubkySession, path: &str, body: &T) -> Result<()>
    where
        T: Serialize + Sync + ?Sized,
    {
        session.storage().put_json(path, body).await?;
        Ok(())
    }

    /// Write raw bytes through a capability-scoped authenticated session.
    ///
    /// # Errors
    ///
    /// Propagates path validation, authorization, and HTTP errors.
    pub async fn put_bytes(
        &self,
        session: &PubkySession,
        path: &str,
        body: impl Into<Vec<u8>>,
    ) -> Result<()> {
        session.storage().put(path, body.into()).await?;
        Ok(())
    }

    /// Delete a resource through a capability-scoped authenticated session.
    ///
    /// # Errors
    ///
    /// Propagates path validation, authorization, and HTTP errors.
    pub async fn delete(&self, session: &PubkySession, path: &str) -> Result<()> {
        session.storage().delete(path).await?;
        Ok(())
    }

    /// Build a public release-event subscription for one publisher.
    ///
    /// The returned builder can be bounded with `limit`, made live, and then
    /// subscribed. Its cursor is homeserver-local and must not be confused
    /// with the publisher's BEP 44 dataset sequence.
    #[must_use]
    pub fn release_events(
        &self,
        user: &PublicKey,
        cursor: Option<EventCursor>,
    ) -> EventStreamBuilder {
        self.sdk
            .event_stream_for_user(user, cursor)
            .path(RELEASES_PATH)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use pubky::{ClientId, Keypair};
    use pubky_testnet::{EphemeralTestnet, pubky_homeserver::ConnectionString};
    use serde_json::{Value, json};
    use swarm_protocol::{
        InfoHashV1, PublisherId, ReleaseFile, ReleaseV1, SourceAttribution, SubjectRef, TagClaimV1,
        TagOperation, TorrentV1,
    };

    use super::*;

    fn postgres_connection() -> ConnectionString {
        let value = std::env::var("TEST_PUBKY_CONNECTION_STRING").unwrap_or_else(|_| {
            let user = std::env::var("USER").expect("USER must identify the local PostgreSQL role");
            format!("postgres://{user}@127.0.0.1:5432/postgres?pubky-test=true")
        });
        ConnectionString::new(&value).expect("valid PostgreSQL connection string")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[pubky_testnet::test]
    async fn real_testnet_crud_listing_and_event_cursor() {
        let testnet = EphemeralTestnet::builder()
            .postgres(postgres_connection())
            .build()
            .await
            .expect("start official Pubky testnet");
        let sdk = testnet.sdk().expect("testnet SDK");
        let adapter = PubkyAdapter::with_sdk(sdk.clone());
        let signer = sdk.signer(Keypair::random());
        let homeserver = testnet.homeserver_app().public_key();

        signer
            .signup(&homeserver, None)
            .await
            .expect("signup with grant authentication");
        let session = signer
            .signin_blocking(ClientId::new("pubky.swarm").expect("static client id"))
            .await
            .expect("grant-backed sign in");
        let user = session.info().public_key().clone();
        let release_path = format!("{RELEASES_PATH}release-1.json");
        let release = json!({
            "schema": "pubky.swarm/release",
            "version": 1,
            "title": "Test release",
            "btih": "ab".repeat(20)
        });

        adapter
            .put_json(&session, &release_path, &release)
            .await
            .expect("write release JSON");
        let fetched: Value = adapter
            .get_public_json(&user, &release_path)
            .await
            .expect("read release JSON");
        assert_eq!(fetched, release);
        assert_eq!(
            adapter
                .get_public_bytes(&user, &release_path)
                .await
                .expect("read release bytes"),
            Bytes::from(serde_json::to_vec(&release).expect("serialize expected JSON"))
        );

        let listed = adapter
            .list_public(&user, RELEASES_PATH, None, 10)
            .await
            .expect("list release directory");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path.as_str(), release_path);

        let mut events = adapter
            .release_events(&user, None)
            .limit(10)
            .subscribe()
            .await
            .expect("subscribe to release events");
        let event = tokio::time::timeout(Duration::from_secs(10), events.next())
            .await
            .expect("release event timeout")
            .expect("event stream closed without release event")
            .expect("valid release event");
        assert_eq!(event.event_type.as_str(), "PUT");
        assert_eq!(event.resource.path.as_str(), release_path);
        assert!(event.cursor.id() > 0);

        adapter
            .delete(&session, &release_path)
            .await
            .expect("delete release");
        assert!(
            adapter
                .list_public(&user, RELEASES_PATH, None, 10)
                .await
                .expect("list after delete")
                .is_empty()
        );

        let publisher = PublisherId::new(user.clone());
        let shared_release = ReleaseV1::new(
            publisher.clone(),
            1,
            "Shared research file".to_owned(),
            "Acceptance-test payload metadata".to_owned(),
            TorrentV1 {
                info_hash: InfoHashV1::from_bytes([0x42; 20]),
                size: 5,
                files: vec![ReleaseFile {
                    path: "shared.txt".to_owned(),
                    size: 5,
                }],
                trackers: Vec::new(),
            },
            vec!["research".to_owned()],
        )
        .expect("valid shared release");
        let tag_claim = TagClaimV1::new(
            publisher,
            SubjectRef::Torrent(shared_release.torrent_ref()),
            "public-domain".to_owned(),
            TagOperation::Add,
            2,
            1,
            SourceAttribution::Direct,
        )
        .expect("valid public tag claim");
        let tag_path = format!("{TAG_CLAIMS_PATH}{}.json", tag_claim.id());
        adapter
            .put_json(&session, &shared_release.storage_path(), &shared_release)
            .await
            .expect("publish validated release");
        adapter
            .put_json(&session, &tag_path, &tag_claim)
            .await
            .expect("publish tag claim");

        let contact = PubkyAdapter::with_sdk(
            testnet
                .sdk()
                .expect("independent contact SDK on the same testnet"),
        );
        let contact_releases = contact
            .list_public(&user, RELEASES_PATH, None, 10)
            .await
            .expect("contact lists shared releases");
        assert!(
            contact_releases
                .iter()
                .any(|resource| resource.path.as_str() == shared_release.storage_path())
        );
        let received_release: ReleaseV1 = contact
            .get_public_json(&user, &shared_release.storage_path())
            .await
            .expect("contact retrieves validated release");
        let received_claim: TagClaimV1 = contact
            .get_public_json(&user, &tag_path)
            .await
            .expect("contact retrieves publisher tag");
        assert_eq!(received_release, shared_release);
        assert_eq!(received_claim, tag_claim);

        adapter
            .delete(&session, &shared_release.storage_path())
            .await
            .expect("delete shared release");
        adapter
            .delete(&session, &tag_path)
            .await
            .expect("delete shared tag");
    }
}
