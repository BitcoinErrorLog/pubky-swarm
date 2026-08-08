#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )); then
  printf 'usage: %s ARTIFACT_DIRECTORY\n' "$0" >&2
  exit 64
fi

artifact_directory="$(cd "$1" && pwd)"
checksum_file="$artifact_directory/SHA256SUMS"

if [[ ! -s "$checksum_file" ]]; then
  printf 'missing checksum manifest: %s\n' "$checksum_file" >&2
  exit 1
fi

(
  cd "$artifact_directory"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum --check SHA256SUMS
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 --check SHA256SUMS
  else
    printf 'neither sha256sum nor shasum is available\n' >&2
    exit 1
  fi
)

if [[ -z "${TAURI_SIGNING_PUBLIC_KEY:-}" ]]; then
  printf 'TAURI_SIGNING_PUBLIC_KEY is required to verify detached signatures\n' >&2
  exit 1
fi

if ! command -v minisign >/dev/null 2>&1; then
  printf 'minisign is required to verify detached signatures\n' >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  printf 'python3 is required to decode Tauri signature material\n' >&2
  exit 1
fi

temporary_directory="$(mktemp -d)"
trap 'rm -rf "$temporary_directory"' EXIT

TAURI_SIGNING_PUBLIC_KEY="$TAURI_SIGNING_PUBLIC_KEY" python3 - "$temporary_directory/public.key" <<'PY'
import base64
import os
import pathlib
import sys

encoded = os.environ["TAURI_SIGNING_PUBLIC_KEY"].strip()
pathlib.Path(sys.argv[1]).write_bytes(base64.b64decode(encoded, validate=True))
PY

verified=0
shopt -s nullglob
for signature in "$artifact_directory"/*.sig; do
  artifact="${signature%.sig}"
  if [[ ! -f "$artifact" ]]; then
    printf 'signature has no matching artifact: %s\n' "$signature" >&2
    exit 1
  fi

  decoded_signature="$temporary_directory/${signature##*/}.decoded"
  python3 - "$signature" "$decoded_signature" <<'PY'
import base64
import pathlib
import sys

encoded = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").strip()
pathlib.Path(sys.argv[2]).write_bytes(base64.b64decode(encoded, validate=True))
PY

  minisign -Vm "$artifact" \
    -p "$temporary_directory/public.key" \
    -x "$decoded_signature"
  verified=$((verified + 1))
done

if (( verified == 0 )); then
  printf 'no detached signatures found in %s\n' "$artifact_directory" >&2
  exit 1
fi

printf 'verified checksums and %d detached signature(s)\n' "$verified"
