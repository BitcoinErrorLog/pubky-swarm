#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
IMAGE="$ROOT/data/PubkySwarmBuild.sparsebundle"
LINK="$ROOT/.target-apfs"
VOLUME="/Volumes/PubkySwarmBuild"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "APFS build volume is only required on macOS external filesystems."
  exit 0
fi

mkdir -p "$ROOT/data"
if [ ! -e "$IMAGE" ]; then
  hdiutil create -size 30g -type SPARSEBUNDLE -fs APFS \
    -volname PubkySwarmBuild "$IMAGE"
fi

if [ ! -d "$VOLUME" ]; then
  hdiutil attach "$IMAGE" -nobrowse >/dev/null
fi

if [ -L "$LINK" ]; then
  current=$(readlink "$LINK")
  if [ "$current" != "$VOLUME" ]; then
    echo "Unexpected build-volume symlink target: $current" >&2
    exit 1
  fi
elif [ -e "$LINK" ]; then
  echo "$LINK exists and is not a symlink; refusing to replace it." >&2
  exit 1
else
  ln -s "$VOLUME" "$LINK"
fi

echo "Build volume ready at $LINK"
