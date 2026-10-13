#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY" >&2
  exit 2
fi

destination="$(realpath -m "$1")"
mkdir -p "$destination"
for target in linuxdeploy-x86_64.AppImage runtime-x86_64; do
  if [[ -e "$destination/$target" || -L "$destination/$target" ]]; then
    echo "refusing to overwrite existing tool path: $destination/$target" >&2
    exit 1
  fi
done

curl_args=(
  --proto '=https'
  --proto-redir '=https'
  --tlsv1.2
  --fail
  --location
  --retry 3
  --retry-all-errors
  --silent
  --show-error
)

curl "${curl_args[@]}" \
  https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage \
  --output "$destination/linuxdeploy-x86_64.AppImage"
curl "${curl_args[@]}" \
  https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
  --output "$destination/runtime-x86_64"

(
  cd "$destination"
  echo 'c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d  linuxdeploy-x86_64.AppImage' | sha256sum --check
  echo '2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d  runtime-x86_64' | sha256sum --check
)
chmod +x "$destination/linuxdeploy-x86_64.AppImage"
