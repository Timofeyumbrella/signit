#!/usr/bin/env bash
# Downloads the prebuilt PDFium library into vendor/pdfium.
#
# The build number must match the pdfium-render API feature in Cargo.toml
# (pdfium-render 0.9.4 targets chromium/7881). Binaries come from
# https://github.com/bblanchon/pdfium-binaries and are checked against the
# SHA-256 digests GitHub publishes for that release.
set -euo pipefail

BUILD=7881

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  asset=pdfium-mac-arm64.tgz;   sha=52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40 ;;
  Darwin-x86_64) asset=pdfium-mac-x64.tgz;     sha=6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b ;;
  Linux-x86_64)  asset=pdfium-linux-x64.tgz;   sha=1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d ;;
  Linux-aarch64) asset=pdfium-linux-arm64.tgz; sha=ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674 ;;
  *) echo "No pinned PDFium build for $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/vendor/pdfium"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${BUILD}/${asset}" -o "$tmp/$asset"
echo "$sha  $tmp/$asset" | shasum -a 256 -c - >/dev/null

rm -rf "$dest"
mkdir -p "$dest"
tar -xzf "$tmp/$asset" -C "$dest"
echo "PDFium chromium/${BUILD} installed in ${dest#"$root"/}/lib"
