#!/usr/bin/env bash
set -euo pipefail

if (( $# == 0 )); then
  printf 'usage: %s ENVIRONMENT_VARIABLE [...]\n' "$0" >&2
  exit 64
fi

missing=()
for name in "$@"; do
  if [[ ! "$name" =~ ^[A-Z][A-Z0-9_]*$ ]]; then
    printf 'invalid environment variable name: %s\n' "$name" >&2
    exit 64
  fi

  if [[ -z "${!name:-}" ]]; then
    missing+=("$name")
  fi
done

if (( ${#missing[@]} > 0 )); then
  printf 'release blocked: required production values are absent:\n' >&2
  printf '  - %s\n' "${missing[@]}" >&2
  exit 1
fi

printf 'release secret preflight passed for %d required values\n' "$#"
