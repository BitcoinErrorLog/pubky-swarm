#!/usr/bin/env bash
set -euo pipefail

if (( $# != 2 )); then
  printf 'usage: %s PLATFORM OUTPUT_DIRECTORY\n' "$0" >&2
  exit 64
fi

platform="$1"
output_directory="$2"
bundle_directory="${CARGO_TARGET_DIR:-target}/release/bundle"

case "$platform" in
  linux)
    accepted_pattern='\.(AppImage|deb)$'
    ;;
  macos)
    accepted_pattern='\.(dmg|tar\.gz)$'
    ;;
  windows)
    accepted_pattern='\.(exe|msi)$'
    ;;
  *)
    printf 'unsupported platform: %s\n' "$platform" >&2
    exit 64
    ;;
esac

if [[ ! -d "$bundle_directory" ]]; then
  printf 'bundle directory does not exist: %s\n' "$bundle_directory" >&2
  exit 1
fi

mkdir -p "$output_directory"
count=0

while IFS= read -r -d '' artifact; do
  filename="${artifact##*/}"

  [[ "$filename" =~ $accepted_pattern ]] || continue
  if [[ -e "$output_directory/$filename" ]]; then
    printf 'duplicate artifact filename: %s\n' "$filename" >&2
    exit 1
  fi
  cp "$artifact" "$output_directory/$filename"
  count=$((count + 1))
done < <(find "$bundle_directory" -type f -print0)

if (( count == 0 )); then
  printf 'no release artifacts found for %s under %s\n' "$platform" "$bundle_directory" >&2
  exit 1
fi

printf 'collected %d %s release artifact(s)\n' "$count" "$platform"
