# quick-xml 0.38.4 security backport

This directory starts from the exact `quick-xml` 0.38.4 crates.io source
(upstream commit `595033e6d1b8078c15da89ed6acf2ae6b45b1918`). The upstream MIT
license and package notices are preserved.

It backports the security behavior from these official quick-xml commits:

- `07f3db8343cf152f5bc3483ef5b3164582489bea` (RUSTSEC-2026-0194):
  switch duplicate-attribute detection to a hash pre-filter after 32
  attributes while preserving exact duplicate positions.
- `7ca25266e94987210daa864889ab15c9332c8a2a` (RUSTSEC-2026-0195):
  reject more than 256 namespace declarations on one element before allocating
  another namespace binding, and expose the same resolver configuration access
  added upstream.

Differences from the 0.41 implementation are limited to line/context adaptation
for the 0.38.4 source and tests. The normalized manifest has an empty
`[workspace]` table so this vendored crate can run its own upstream test suite
beneath the parent workspace. No package version, feature, dependency, license,
or unrelated runtime implementation was changed.
