#!/usr/bin/env sh
# Builds the release cdylib and compiles tests/c/consumer_smoke.c against
# include/trend.h with a C11 compiler, then runs it. Linux and macOS.
set -eu
cd "$(dirname "$0")/.."
cargo build --release --lib
target_dir="${CARGO_TARGET_DIR:-target}"
out="$target_dir/c-consumer-smoke"
${CC:-cc} -std=c11 -Wall -Wextra -Werror -pedantic -Iinclude tests/c/consumer_smoke.c \
    -L"$target_dir/release" -ltrend -o "$out"
LD_LIBRARY_PATH="$target_dir/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
DYLD_LIBRARY_PATH="$target_dir/release${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
    "$out"
