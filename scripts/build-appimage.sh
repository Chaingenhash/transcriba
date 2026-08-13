#!/usr/bin/env bash
#
# Builds the Linux AppImage the same way the release workflow does.
#
#   ./scripts/build-appimage.sh            # portable: safe to give to anyone
#   NATIVE=1 ./scripts/build-appimage.sh   # tuned for THIS machine only
#
# Every flag below is load-bearing. If you are tempted to drop one, read its
# comment first — each of them cost a real failure to find.
set -euo pipefail

cd "$(dirname "$0")/.."

# linuxdeploy's vendored `strip` predates the SHT_RELR/.relr.dyn compact
# relocation format current binutils emits, so it chokes on system libraries
# (webkit2gtk, gtk, glib) with "unknown type [0x13] section '.relr.dyn'" and
# tauri reports the unhelpful "failed to run linuxdeploy". Costs us an
# unstripped bundle (~104MB) until a newer linuxdeploy is vendored.
export NO_STRIP=1

# whisper-rs-sys never passes GGML_NATIVE, so ggml defaults it ON and compiles
# libwhisper with `-march=native` — for whichever CPU did the build. Shipping
# that means a colleague on an older machine gets `SIGILL: illegal instruction`
# during model load instead of a transcript, with nothing useful on screen.
# OFF selects ggml's explicit baseline (SSE4.2/AVX/AVX2/BMI2/FMA/F16C, no
# AVX512, no AMX), which is Haswell-era and portable.
#
# NATIVE=1 opts back in. Only do that for a build you will run on this machine
# and hand to nobody.
if [[ "${NATIVE:-0}" == "1" ]]; then
  echo "warning: building with -march=native — this AppImage may crash on other machines" >&2
else
  export GGML_NATIVE=OFF
fi

# Vulkan is enabled for Linux because it measured 3.9x faster than CPU on Intel
# integrated graphics (110s vs 431s for a 5-minute clip). It degrades safely: a
# machine with no usable Vulkan device falls back to CPU, and the finished
# document's header names whichever actually ran. Needs libvulkan-dev and glslc
# at build time.
FEATURES=(--features vulkan)

# Changing the instruction-set flags does not invalidate cargo's fingerprint for
# the already-built libwhisper — whisper-rs-sys emits no rerun-if-env-changed
# for GGML_* — so a stale artifact would survive and silently defeat the setting
# above. Drop just that crate's build output rather than the whole target dir.
if [[ "${CLEAN_WHISPER:-1}" == "1" ]]; then
  cargo clean -p whisper-rs-sys 2>/dev/null || true
fi

cd app
npm ci --silent
npx tauri build --bundles appimage "${FEATURES[@]}"

cd ..
BUNDLE=$(find target/release/bundle/appimage -name '*.AppImage' -print -quit)
echo
echo "AppImage: $BUNDLE"
echo "Size:     $(du -h "$BUNDLE" | cut -f1)"
echo
echo "libfuse2 is not installed here, so run it as:"
echo "  $BUNDLE --appimage-extract-and-run"
