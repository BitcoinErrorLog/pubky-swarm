#!/usr/bin/env bash
set -euo pipefail

readonly COMMIT="7d7fc38fac61177fa5e02148f791b2f65250b09d"
readonly VERSION="2.0.13"
readonly SHA256="892cb75c06318e2420de0faf9f63a908069d3d237676e2459fd30abe0cb3b1bf"
readonly URL="https://github.com/arvidn/libtorrent/releases/download/v${VERSION}/libtorrent-rasterbar-${VERSION}.tar.gz"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
VENDOR_DIR="${WORKSPACE_ROOT}/vendor/libtorrent"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/libtorrent-${VERSION}.XXXXXX")"
readonly SCRIPT_DIR WORKSPACE_ROOT VENDOR_DIR TEMP_DIR
trap 'rm -rf "${TEMP_DIR}"' EXIT

curl --fail --location --silent --show-error \
  "${URL}" \
  --output "${TEMP_DIR}/source.tar.gz"

if command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(shasum -a 256 "${TEMP_DIR}/source.tar.gz" | awk '{print $1}')"
elif command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(sha256sum "${TEMP_DIR}/source.tar.gz" | awk '{print $1}')"
else
  echo "A SHA-256 tool (shasum or sha256sum) is required." >&2
  exit 1
fi
readonly ACTUAL_SHA256

if [[ "${ACTUAL_SHA256}" != "${SHA256}" ]]; then
  echo "libtorrent archive checksum mismatch." >&2
  echo "expected: ${SHA256}" >&2
  echo "actual:   ${ACTUAL_SHA256}" >&2
  exit 1
fi

mkdir -p "${TEMP_DIR}/source"
tar -xzf "${TEMP_DIR}/source.tar.gz" \
  --strip-components=1 \
  -C "${TEMP_DIR}/source"

test -f "${TEMP_DIR}/source/CMakeLists.txt"
grep -Fq '#define LIBTORRENT_VERSION "2.0.13.0"' \
  "${TEMP_DIR}/source/include/libtorrent/version.hpp"

rm -rf "${VENDOR_DIR}"
mkdir -p "$(dirname "${VENDOR_DIR}")"
mv "${TEMP_DIR}/source" "${VENDOR_DIR}"

echo "Vendored official libtorrent ${VERSION} commit ${COMMIT}."
echo "Verified SHA-256 ${SHA256}."
