# ADR 0002: Mutable Dataset Torrent Head

## Status

Accepted for the v1 experiment.

## Decision

Use BEP 46 over BEP 44 as the mutable pointer to the current dataset torrent.
The publisher is the same Ed25519 key as the Pubky identity. Use the fixed salt:

```text
pubky.swarm/v1/dataset
```

The signed value is the canonical BEP 46 dictionary:

```text
d2:ih20:<20 raw infohash bytes>e
```

Do not add `_swarm` to the PKARR DNS packet in V0.

## Evidence

`swarm-head` verifies:

- official BEP 46 salted target vectors;
- identical public keys from one seed in Pubky and Mainline;
- canonical value encoding;
- local five-node DHT publication and resolution;
- monotonic sequence updates with CAS;
- stale expected-sequence rejection;
- persisted highest-seen rollback checks; and
- reannouncement of an unchanged signed item without its private key.

## Rationale

PKARR has one unsalted mutable DNS packet per Pubky identity. Adding a Swarm
record would require every independent writer to read, preserve, merge, sign,
and republish every other DNS record inside the shared 1000-byte budget.

BEP 46 already standardizes “public key plus optional salt points to current
torrent.” The salt gives Swarm an independent DHT slot while retaining the
Pubky identity as authority.

## Security properties and limits

- DHT nodes verify the root Pubky signature, sequence, and target.
- CAS reduces concurrent lost updates but is not global consensus.
- Clients persist the highest accepted sequence and reject lower values.
- A first-contact client cannot prove that a valid signed response is globally
  latest.
- Mutable items need periodic reannouncement; any holder can reannounce the
  existing signed bytes, but only the root signer can publish a new state.
- Dataset object authentication remains the manifest's responsibility; the
  BEP 46 value only authenticates the current torrent identity.
