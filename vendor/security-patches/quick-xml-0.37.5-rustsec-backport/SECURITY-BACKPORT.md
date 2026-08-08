# quick-xml 0.37.5 security backport

This directory starts from the exact `quick-xml` 0.37.5 crates.io source
(upstream commit `a018365abc4743b4abbd53892a7362b3f239bbd1`). The upstream MIT
license and package notices are preserved.

It backports the security behavior from these official quick-xml commits:

- `07f3db8343cf152f5bc3483ef5b3164582489bea` (RUSTSEC-2026-0194):
  switch duplicate-attribute detection to a hash pre-filter after 32
  attributes while preserving exact duplicate positions.
- `7ca25266e94987210daa864889ab15c9332c8a2a` (RUSTSEC-2026-0195):
  reject more than 256 namespace declarations on one element before allocating
  another namespace binding.

Backport differences from the 0.41 implementation are limited to API adaptation:

- 0.37.5 keeps `NamespaceResolver` private, so the public 0.41 resolver
  configuration methods are not exposed. The secure limit remains fixed at the
  upstream default of 256 in production.
- Tests are expressed using the 0.37.5 attribute and namespace APIs.
- The normalized manifest has an empty `[workspace]` table so this vendored
  crate can run its own upstream test suite beneath the parent workspace.

No package version, feature, dependency, license, or unrelated runtime
implementation was changed.
