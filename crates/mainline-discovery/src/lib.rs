//! Mainline DHT peer discovery independent of any torrent transfer engine.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::net::{SocketAddr, SocketAddrV4};
use std::time::Duration;

use futures::StreamExt;
use mainline::async_dht::AsyncDht;
use swarm_protocol::InfoHashV1;

/// Peer-discovery errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Port zero cannot be announced explicitly.
    #[error("cannot announce peer port 0")]
    InvalidPort,
    /// Mainline rejected or could not complete an announcement.
    #[error("Mainline announce failed: {0}")]
    Announce(String),
    /// No peers were found before the caller's deadline.
    #[error("peer lookup timed out")]
    Timeout,
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Peer announce and lookup over an injected Mainline DHT.
#[derive(Debug, Clone)]
pub struct PeerDiscovery {
    dht: AsyncDht,
}

impl PeerDiscovery {
    /// Construct from either a local testnet or public Mainline client.
    #[must_use]
    pub const fn new(dht: AsyncDht) -> Self {
        Self { dht }
    }

    /// Announce this process as a peer for `info_hash`.
    ///
    /// # Errors
    ///
    /// Rejects port zero and propagates Mainline announce failures.
    pub async fn announce(&self, info_hash: InfoHashV1, port: u16) -> Result<()> {
        if port == 0 {
            return Err(Error::InvalidPort);
        }
        self.dht
            .announce_peer(info_hash_id(info_hash), Some(port))
            .await
            .map_err(|error| Error::Announce(error.to_string()))?;
        Ok(())
    }

    /// Complete one DHT peer lookup and return unique peers in stable order.
    #[must_use]
    pub async fn lookup(&self, info_hash: InfoHashV1) -> Vec<SocketAddrV4> {
        self.dht
            .get_peers(info_hash_id(info_hash))
            .fold(BTreeSet::new(), |mut peers, batch| async move {
                peers.extend(batch);
                peers
            })
            .await
            .into_iter()
            .collect()
    }

    /// Retry DHT lookups until at least one peer is found or the deadline
    /// expires.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Timeout`] when no peers are found before `timeout`.
    pub async fn wait_for_peers(
        &self,
        info_hash: InfoHashV1,
        timeout: Duration,
    ) -> Result<Vec<SocketAddr>> {
        let lookup = async {
            loop {
                let peers = self.lookup(info_hash).await;
                if !peers.is_empty() {
                    return peers.into_iter().map(SocketAddr::V4).collect();
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(timeout, lookup)
            .await
            .map_err(|_| Error::Timeout)
    }
}

fn info_hash_id(info_hash: InfoHashV1) -> mainline::Id {
    (*info_hash.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use mainline::{Dht, Testnet};

    use super::*;

    fn client(bootstrap: &[String]) -> PeerDiscovery {
        PeerDiscovery::new(
            Dht::builder()
                .bootstrap(bootstrap)
                .bind_address(Ipv4Addr::LOCALHOST)
                .build()
                .expect("local DHT client")
                .as_async(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn isolated_testnet_announces_and_resolves_peer() {
        let testnet = Testnet::builder(5).build().expect("local Mainline testnet");
        let announcer = client(&testnet.bootstrap);
        let reader = client(&testnet.bootstrap);
        let info_hash = InfoHashV1::from_bytes([0x51; 20]);
        let port = 45_678;

        announcer
            .announce(info_hash, port)
            .await
            .expect("announce generated torrent");
        let peers = reader
            .wait_for_peers(info_hash, Duration::from_secs(10))
            .await
            .expect("resolve announced peer");
        assert!(peers.contains(&SocketAddr::from((Ipv4Addr::LOCALHOST, port))));
    }

    #[tokio::test]
    async fn rejects_zero_port() {
        let testnet = Testnet::builder(3).build().expect("local Mainline testnet");
        let discovery = client(&testnet.bootstrap);
        assert!(matches!(
            discovery.announce(InfoHashV1::from_bytes([0; 20]), 0).await,
            Err(Error::InvalidPort)
        ));
    }
}
