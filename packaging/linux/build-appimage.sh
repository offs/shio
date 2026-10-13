#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

if [[ $# -ne 1 || -z "${LINUXDEPLOY:-}" || -z "${APPIMAGE_RUNTIME:-}" || -z "${VERSION:-}" ]]; then
  echo "usage: LINUXDEPLOY=... APPIMAGE_RUNTIME=... VERSION=... $0 OUTPUT" >&2
  exit 2
fi
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct HEAD)}"

desktop=packaging/linux/com.shio.DownloadManager.desktop
metainfo=packaging/linux/com.shio.DownloadManager.metainfo.xml
output="$(realpath -m "$1")"
zsync_name="$(basename "$output").zsync"
update_information="gh-releases-zsync|offs|shio|latest|$zsync_name"
workdir="$(mktemp -d)"
trap 'rm -rf -- "$workdir"' EXIT
appdir="$workdir/Shio.AppDir"

desktop-file-validate "$desktop"
appstreamcli validate --no-net "$metainfo"
if ! grep -Fq "<release version=\"$VERSION\"" "$metainfo"; then
  echo "$metainfo has no release entry for $VERSION" >&2
  exit 1
fi

for size in 256 64 48; do
  install -D -m 644 "assets/icons/icon-$size.png" "$workdir/icons/$size/com.shio.DownloadManager.png"
done
install -D -m 644 -t "$appdir/usr/share/doc/shio" LICENSE packaging/THIRD_PARTY_LICENSES.md
install -D -m 644 -t "$appdir/usr/share/metainfo" "$metainfo"
mkdir -p "$(dirname "$output")"

# The metadata was validated offline above; appimagetool's own check needs the network.
APPIMAGE_EXTRACT_AND_RUN=1 \
NO_STRIP=1 \
LINUXDEPLOY_OUTPUT_VERSION="$VERSION" \
LDAI_RUNTIME_FILE="$APPIMAGE_RUNTIME" \
LDAI_OUTPUT="$output" \
LDAI_NO_APPSTREAM=1 \
LDAI_UPDATE_INFORMATION="$update_information" \
  "$LINUXDEPLOY" \
    --appdir "$appdir" \
    --executable target/release/shio \
    --desktop-file "$desktop" \
    --icon-file "$workdir/icons/256/com.shio.DownloadManager.png" \
    --icon-file "$workdir/icons/64/com.shio.DownloadManager.png" \
    --icon-file "$workdir/icons/48/com.shio.DownloadManager.png" \
    --output appimage
# appimagetool writes the .zsync file into the working directory.
[[ "$PWD/$zsync_name" == "$output.zsync" ]] || mv -- "$zsync_name" "$output.zsync"

(cd "$workdir" && "$output" --appimage-extract >/dev/null)
# THIRD_PARTY_LICENSES.md covers fonts and Rust crates only; a bundled native library needs its own notice.
bundled="$(find "$workdir/squashfs-root/usr/lib" -type f -name '*.so*' 2>/dev/null || true)"
if [[ -n "$bundled" ]]; then
  echo "AppImage bundles native libraries without notices:" >&2
  echo "$bundled" >&2
  exit 1
fi
