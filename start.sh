#!/bin/sh

ROOT=$(CDPATH= cd "$(dirname "$0")" && pwd)
cd "$ROOT" || exit 1

export PATH="$ROOT/target/release${PATH:+:$PATH}"
XDG_DATA_DIRS=${XDG_DATA_DIRS:-/usr/local/share:/usr/share}
export XDG_DATA_DIRS="$ROOT/utils/spacelauncher/data:$ROOT/utils/spacesettings/data:$ROOT/utils/cursor-position/data:$XDG_DATA_DIRS"

cargo build --release --workspace
exec "$ROOT/target/release/spacetop" --launcher="$ROOT/target/release/spacelauncher"