#!/usr/bin/env bash
set -euo pipefail

if (( $# < 1 )); then
  printf 'usage: %s ARTIFACT_DIRECTORY [ARTIFACT_NAME ...]\n' "$0" >&2
  exit 64
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

"$repo_root/scripts/release/require-release-secrets.sh" \
  TAURI_SIGNING_PRIVATE_KEY \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD

if [[ ! -d "$1" ]]; then
  printf 'artifact directory does not exist: %s\n' "$1" >&2
  exit 1
fi
artifact_directory="$(cd "$1" && pwd)"

shift
if (( $# == 0 )); then
  shopt -s nullglob
  artifacts=("$artifact_directory"/*)
else
  artifacts=()
  for name in "$@"; do
    if [[ "$name" == */* || "$name" == "." || "$name" == ".." ]]; then
      printf 'artifact name must be a basename: %s\n' "$name" >&2
      exit 64
    fi
    artifacts+=("$artifact_directory/$name")
  done
fi
signed=0

for artifact in "${artifacts[@]}"; do
  if [[ ! -f "$artifact" ]]; then
    printf 'artifact does not exist: %s\n' "$artifact" >&2
    exit 1
  fi
  case "$artifact" in
    *.sig)
      continue
      ;;
  esac

  npm --prefix "$repo_root/apps/desktop" exec -- \
    tauri signer sign "$artifact"

  if [[ ! -s "$artifact.sig" ]]; then
    printf 'signer did not produce a signature for %s\n' "$artifact" >&2
    exit 1
  fi
  signed=$((signed + 1))
done

if (( signed == 0 )); then
  printf 'no artifacts were available to sign in %s\n' "$artifact_directory" >&2
  exit 1
fi

printf 'created detached signatures for %d artifact(s)\n' "$signed"
