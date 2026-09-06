#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --workspace --all-features --release

package="dist/linux"
rm -rf "$package"
mkdir -p "$package/bin" "$package/include" "$package/lib" "$package/samples" "$package/docs"
cp target/release/trend-test-viewer "$package/bin/"
cp target/release/trend-python-tui-bridge "$package/bin/"
cp target/release/libtrend.so "$package/lib/"
cp include/trend.h "$package/include/"
cp tests/fixtures/media/ColorTest.png tests/fixtures/media/ColorTest.gif "$package/samples/"
cp docs/bridge-protocol.md "$package/docs/"
cp packaging/linux/README.md "$package/README.md"
cp LICENSE NOTICE THIRD_PARTY_NOTICES.md "$package/"

(
    cd "$package"
    find . -type f ! -name manifest.sha256 -print0 \
        | sort -z \
        | xargs -0 sha256sum \
        | sed 's#  \./#  #' > manifest.sha256
)

"$package/bin/trend-test-viewer" --version >/dev/null
"$package/bin/trend-python-tui-bridge" --version >/dev/null
"$package/bin/trend-test-viewer" --probe 20x10 2 "$package/samples/ColorTest.png" >/dev/null

echo "Linux package: $package"