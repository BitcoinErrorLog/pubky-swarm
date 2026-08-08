#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )); then
  printf 'usage: %s ARTIFACT_DIRECTORY\n' "$0" >&2
  exit 64
fi

artifact_directory="$1"
checksum_file="$artifact_directory/SHA256SUMS"

if [[ ! -d "$artifact_directory" ]]; then
  printf 'artifact directory does not exist: %s\n' "$artifact_directory" >&2
  exit 1
fi

shopt -s nullglob
artifacts=("$artifact_directory"/*)
names=()

for artifact in "${artifacts[@]}"; do
  [[ -f "$artifact" ]] || continue
  [[ "$artifact" == "$checksum_file" ]] && continue
  names+=("${artifact##*/}")
done

if (( ${#names[@]} == 0 )); then
  printf 'no artifacts available for checksums in %s\n' "$artifact_directory" >&2
  exit 1
fi

IFS=$'\n' names=($(printf '%s\n' "${names[@]}" | LC_ALL=C sort))
unset IFS

(
  cd "$artifact_directory"
  : > SHA256SUMS
  for name in "${names[@]}"; do
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$name" >> SHA256SUMS
    elif command -v shasum >/dev/null 2>&1; then
      shasum -a 256 "$name" >> SHA256SUMS
    else
      printf 'neither sha256sum nor shasum is available\n' >&2
      exit 1
    fi
  done
)

printf 'wrote checksums for %d artifact(s) to %s\n' "${#names[@]}" "$checksum_file"
