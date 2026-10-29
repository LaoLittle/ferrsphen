#!/usr/bin/env bash
# Build Ferrsphen.app and Ferrsphen.ipa.
#
#   ios/package.sh [--simulator] [--release]
#
# Signing: ad-hoc by default, which runs on the iOS Simulator only. For devices set
# CODESIGN_IDENTITY (e.g. "Apple Development: …") and PROVISIONING_PROFILE (path to a
# .mobileprovision whose App ID matches dev.ferrsphen.app).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
target=aarch64-apple-ios
profile=debug
cargo_profile=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --simulator) target=aarch64-apple-ios-sim ;;
        --release) profile=app-release; cargo_profile=(--profile app-release) ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

cd "$root"
export IPHONEOS_DEPLOYMENT_TARGET=16.0
cargo build --locked --target "$target" --bin ferrsphen ${cargo_profile[@]+"${cargo_profile[@]}"}

version="$(cargo metadata --format-version 1 --no-deps | python3 -c \
    'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="ferrsphen"))')"
target_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c \
    'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
output="$target_dir/ios/$target/$profile"
app="$output/Payload/Ferrsphen.app"
rm -rf "$output"
mkdir -p "$app"
cp "$target_dir/$target/$profile/ferrsphen" "$app/ferrsphen"
cp ios/Info.plist "$app/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" -c "Set :CFBundleVersion $version" "$app/Info.plist"
if [[ "$target" == *-sim ]]; then
    /usr/libexec/PlistBuddy -c "Add :CFBundleSupportedPlatforms array" \
        -c "Add :CFBundleSupportedPlatforms:0 string iPhoneSimulator" "$app/Info.plist"
else
    /usr/libexec/PlistBuddy -c "Add :CFBundleSupportedPlatforms array" \
        -c "Add :CFBundleSupportedPlatforms:0 string iPhoneOS" "$app/Info.plist"
fi
plutil -convert binary1 "$app/Info.plist"
cp LICENSE* "$app/"

identity="${CODESIGN_IDENTITY:--}"
sign=(codesign --force --sign "$identity" --timestamp=none)
if [[ -n "${PROVISIONING_PROFILE:-}" ]]; then
    cp "$PROVISIONING_PROFILE" "$app/embedded.mobileprovision"
    security cms -D -i "$PROVISIONING_PROFILE" > "$output/profile.plist"
    /usr/libexec/PlistBuddy -x -c "Print :Entitlements" "$output/profile.plist" > "$output/entitlements.plist"
    sign+=(--entitlements "$output/entitlements.plist")
fi
"${sign[@]}" "$app"
codesign --verify --strict "$app"

(cd "$output" && zip -qry Ferrsphen.ipa Payload)
echo "$app"
echo "$output/Ferrsphen.ipa"
