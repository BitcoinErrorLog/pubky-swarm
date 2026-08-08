# ADR 0003: Root-Key Signing Boundary

## Status

Lab mode accepted; production flow blocked on signer support.

## Decision

Normal application publishing uses Pubky 0.10 grant/PoP sessions and never
receives the root key.

Phase 2 lab demonstrations may use a newly generated, isolated test identity
held by the Rust backend. They must not request a user's primary Pubky recovery
key.

Production BEP 46 updates require explicit live signing by a trusted root-key
device such as Pubky Ring. Pubky Noise may later transport those requests, but
it is not itself a delegation protocol.

## Why Pubky Noise is not delegation

`pubky-noise 0.1.0-rc5` provides an authenticated encrypted peer channel over
Homeserver outboxes. It does not define signing scope, consent, expiration,
revocation, monotonic request counters, or audit receipts. Its current
`PubkyNoiseConfig` also takes the root secret and targets Pubky 0.8, so embedding
it unchanged in the desktop backend would violate the custody boundary.

## Required signing request

A future signer protocol must present and bind, at minimum:

- protocol and schema version;
- exact BEP 46 salt;
- previous and next sequence;
- exact canonical value bytes and decoded infohash;
- requesting application/device identity;
- expiry and unique request nonce; and
- visible user confirmation or an explicit narrowly scoped policy.

The response must bind the request hash, signature, signer identity, and result.
Replay, sequence regression, salt substitution, and value substitution must be
rejected.

## Operational consequence

The root signer is needed only when publishing a new dataset state. Seeders can
refresh the already signed BEP 44 item and torrent bytes without signing
authority.
