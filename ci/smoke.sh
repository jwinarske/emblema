#!/usr/bin/env bash
# Verification run: format, lint, build, test, and the feature matrix.
#
# Every step's exit status is captured directly rather than through a pipe.
# Piping cargo into `tail` hands the pipeline `tail`'s status, which is always
# zero, so a broken build reports success -- this script previously did exactly
# that and passed while the workspace did not compile.
set -uo pipefail

cd "$(dirname "$0")/.."
fail=0

step() {
    local name="$1"
    shift
    local output
    if output=$("$@" 2>&1); then
        printf '  %-28s ok\n' "$name"
    else
        printf '  %-28s FAILED\n' "$name"
        printf '%s\n' "$output" | tail -25 | sed 's/^/      /'
        fail=1
    fi
}

echo "verification"
step "rustfmt" cargo fmt --all -- --check
step "clippy" cargo clippy --workspace --all-targets -- -D warnings
step "build" cargo build --workspace --all-targets

# Through xtask rather than `cargo test` directly, because a skipped test
# passes and `cargo test` captures the reason along with the rest of a passing
# test's output. This runs the suite uncaptured and prints the census, so a run
# says how much of itself ran rather than only that it was green. Its output is
# shown whether or not it succeeded, which is the point: a clean run with six
# skips and a clean run with none look identical without it.
echo "tests"
if census=$(cargo xtask verify 2>&1); then
    printf '%s\n' "$census" | sed 's/^/  /'
else
    printf '%s\n' "$census" | tail -30 | sed 's/^/  /'
    fail=1
fi

# The example and the gallery are code that runs, and neither is covered by the
# suite: a change that breaks one compiles, passes every test, and is found by
# whoever next runs it. The gallery is also the only thing here that can see a
# scene both backends get wrong the same way, so a broken one is a check
# quietly lost rather than an inconvenience.
echo "artifacts"
step "example" cargo run -q -p emblema --example frame -- "$(mktemp -u).ppm"
step "gallery" cargo run -q -p xtask -- gallery "$(mktemp -u).ppm"

echo "feature matrix"
for features in vulkan gles vulkan,gles,drm present-wsi gles,present-egl vulkan,gles,drm,present-wsi,present-egl; do
    step "$features" cargo check -p emblema --no-default-features --features "$features"
done

if [ "$fail" -eq 0 ]; then
    echo "SMOKE: PASS"
else
    echo "SMOKE: FAIL"
fi
exit "$fail"
