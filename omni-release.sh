#!/bin/bash
# Builds the macOS Gyroflow (Omni) plugins and uploads them to the draft GitHub release.
#
# Omni has no CI, so releases are built locally on each OS:
#   1. Windows: .\omni-release.ps1          builds, creates the draft release v<version>, uploads the Windows files
#   2. macOS:   ./omni-release.sh           builds and uploads the macOS files to the same draft
#   3. Either:  ./omni-release.sh --publish checks every file is there, then publishes the release
# Either OS can go first; whichever runs first creates the draft.
#
# Options:
#   --notes FILE   Markdown release notes (shown in the app's update popup); default: generated from commits
#   --publish      publish the existing draft instead of building
#   --skip-build   upload the files already in target/ without rebuilding
#   --dry-run      build and check the files, but don't change anything on GitHub
#   --adhoc        allow an ad-hoc signed, un-notarized build (no Developer ID); browser downloads then need
#                  xattr -dr com.apple.quarantine <bundle>
#
# Signing and notarization (one-time setup on the Mac):
#   security find-identity -v -p codesigning      # copy the hash of your "Developer ID Application" certificate
#   xcrun notarytool store-credentials omni-notary --apple-id <your Apple ID> --team-id <team ID>
#                                                  # asks for an app-specific password from account.apple.com
# Then each release:
#   ./omni-release.sh   # finds your Developer ID; SIGNING_FINGERPRINT=<hash> picks one; NOTARY_PROFILE defaults to omni-notary
# The plugins are signed by the build recipes, then the zips and dmgs are notarized and the dmgs stapled.
set -euo pipefail

REPO="yshui1/gyroflow-plugins-omni"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
TARGET="$ROOT/target"
# Files the app's plugin installer downloads; keep in sync with gyroflow-omni src/nle_plugins.rs
WINDOWS_ASSETS=(Gyroflow-OpenFX-windows.zip Gyroflow-Adobe-windows.aex)
MAC_ASSETS=(Gyroflow-OpenFX-macos.zip Gyroflow-Adobe-macos.zip)
# Installers for people who download the plugins by hand; uploaded when present, not required to publish
MAC_EXTRAS=(Gyroflow-OpenFX-macos.dmg Gyroflow-Adobe-macos.dmg)
NOTARY_PROFILE="${NOTARY_PROFILE:-omni-notary}"

NOTES=""; PUBLISH=0; SKIP_BUILD=0; DRY_RUN=0; ADHOC=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --notes) NOTES="$(cd "$(dirname "$2")" && pwd -P)/$(basename "$2")"; shift 2 ;;
        --publish) PUBLISH=1; shift ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        --adhoc) ADHOC=1; shift ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done

die() { echo "Error: $*" >&2; exit 1; }
crate_version() { sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$1" | head -1; }
for tool in gh git; do command -v "$tool" >/dev/null || die "$tool is not installed."; done

notarize() { # submit to Apple's notary service and wait; staple the ticket into dmgs
    local f="$1" out
    echo "Notarizing $(basename "$f") (usually a few minutes)..."
    out="$(xcrun notarytool submit "$f" --keychain-profile "$NOTARY_PROFILE" --wait --output-format json)" || die "notarytool failed for $f: $out"
    grep -Eq '"status" *: *"Accepted"' <<<"$out" || die "Apple rejected $(basename "$f"): $out
Details: xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE"
    if [[ "$f" == *.dmg ]]; then
        xcrun stapler staple "$f"
        spctl --assess --type open --context context:primary-signature -v "$f"
    fi
}

VERSION="$(crate_version "$ROOT/openfx/Cargo.toml")"
[[ -n "$VERSION" ]] || die "No version in openfx/Cargo.toml"
for crate in adobe frei0r; do
    v="$(crate_version "$ROOT/$crate/Cargo.toml")"
    [[ "$v" == "$VERSION" ]] || die "$crate/Cargo.toml is $v but openfx/Cargo.toml is $VERSION. Set them to the same version."
done
TAG="v$VERSION"
echo "Gyroflow (Omni) plugins $TAG"

release_state() { # prints "draft", "published" or "none"
    local d
    if d="$(gh release view "$TAG" -R "$REPO" --json isDraft --jq .isDraft 2>/dev/null)"; then
        [[ "$d" == "true" ]] && echo draft || echo published
    else
        echo none
    fi
}

if [[ $PUBLISH -eq 1 ]]; then
    state="$(release_state)"
    [[ "$state" != none ]] || die "No release $TAG on $REPO. Build and upload first."
    [[ "$state" != published ]] || { echo "$TAG is already published."; exit 0; }
    have="$(gh release view "$TAG" -R "$REPO" --json assets --jq '.assets[].name')"
    missing=()
    for a in "${WINDOWS_ASSETS[@]}" "${MAC_ASSETS[@]}"; do grep -qxF "$a" <<<"$have" || missing+=("$a"); done
    [[ ${#missing[@]} -eq 0 ]] || die "Not publishing, the draft is missing: ${missing[*]}"
    read -r -p "Publish $TAG to everyone (the app will offer it as an update)? [y/N] " answer
    [[ "$answer" =~ ^(y|yes)$ ]] || { echo "Cancelled."; exit 1; }
    gh release edit "$TAG" -R "$REPO" --draft=false --latest
    echo "Published https://github.com/$REPO/releases/tag/$TAG"
    exit 0
fi

# The release tag is created at this commit, so it has to be on GitHub.
cd "$ROOT"
SHA="$(git rev-parse HEAD)"
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || die "There are uncommitted changes. Commit them first so the release matches a commit."
git fetch --quiet origin
[[ -n "$(git branch -r --contains "$SHA")" ]] || die "Commit ${SHA:0:8} isn't pushed to origin. Push it first."

# Check signing and notary credentials before a long build.
# Without SIGNING_FINGERPRINT, use the Keychain's Developer ID Application certificate when there is exactly one.
if [[ -z "${SIGNING_FINGERPRINT:-}" && $ADHOC -eq 0 ]]; then
    ids="$(security find-identity -v -p codesigning | sed -nE 's/^ *[0-9]+\) ([0-9A-F]{40}) "Developer ID Application:.*/\1/p' | sort -u)"
    [[ $(grep -c . <<<"$ids") -eq 1 ]] && SIGNING_FINGERPRINT="$ids" && echo "Signing with Developer ID $SIGNING_FINGERPRINT"
fi
if [[ -n "${SIGNING_FINGERPRINT:-}" ]]; then
    security find-identity -v -p codesigning | grep -q "$SIGNING_FINGERPRINT" || die "No codesigning identity matches SIGNING_FINGERPRINT."
    export SIGNING_FINGERPRINT   # the just recipes sign with it
    # notarytool exits 0 even when the profile is missing, so check what it prints.
    xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" 2>&1 | grep -q "Successfully received submission history"         || die "Notary profile '$NOTARY_PROFILE' not found. Create it: xcrun notarytool store-credentials $NOTARY_PROFILE --apple-id <Apple ID> --team-id <team ID>"
elif [[ $ADHOC -eq 0 ]]; then
    die "SIGNING_FINGERPRINT isn't set and there isn't exactly one Developer ID Application certificate in Keychain. Set it to your certificate's hash, or pass --adhoc for an unsigned test build."
fi

if [[ $SKIP_BUILD -eq 0 ]]; then
    command -v just >/dev/null || die "just is not installed (brew install just)."
    export OMNI_RELEASE=1   # clean version numbers (1.0.0.0 instead of 1.0.0-dev)
    (cd common && cargo test --locked)
    just ofx deploy
    just adobe deploy
    unset OMNI_RELEASE
    if [[ -n "${SIGNING_FINGERPRINT:-}" ]]; then
        for name in "${MAC_ASSETS[@]}" "${MAC_EXTRAS[@]}"; do notarize "$TARGET/$name"; done
    fi
elif [[ -n "${SIGNING_FINGERPRINT:-}" ]]; then
    for name in "${MAC_EXTRAS[@]}"; do
        xcrun stapler validate "$TARGET/$name" >/dev/null || die "$name isn't notarized. Rebuild without --skip-build."
    done
fi

FILES=()
for name in "${MAC_ASSETS[@]}"; do
    [[ -f "$TARGET/$name" ]] || die "Missing $TARGET/$name"
    FILES+=("$TARGET/$name")
done
for name in "${MAC_EXTRAS[@]}"; do [[ -f "$TARGET/$name" ]] && FILES+=("$TARGET/$name"); done
plist="$TARGET/gyroflow-ofx-macos/GyroflowOmni.ofx.bundle/Contents/Info.plist"
ofx_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")"
[[ "$ofx_version" == "$VERSION.0" ]] || die "GyroflowOmni.ofx.bundle reports version $ofx_version, expected $VERSION.0. Rebuild without --skip-build."
for f in "${FILES[@]}"; do
    printf '  %s  %s  SHA256 %s\n' "$(basename "$f")" "$(du -h "$f" | cut -f1)" "$(shasum -a 256 "$f" | cut -d' ' -f1)"
done

if [[ $DRY_RUN -eq 1 ]]; then echo "Dry run: nothing uploaded."; exit 0; fi

state="$(release_state)"
[[ "$state" != published ]] || die "$TAG is already published. Bump the version for a new release."
if [[ "$state" == none ]]; then
    if [[ -n "$NOTES" ]]; then notes=(--notes-file "$NOTES"); else notes=(--generate-notes); fi
    gh release create "$TAG" -R "$REPO" --draft --target "$SHA" --title "Gyroflow (Omni) plugins $TAG" "${notes[@]}"
elif [[ -n "$NOTES" ]]; then
    gh release edit "$TAG" -R "$REPO" --notes-file "$NOTES"
fi
gh release upload "$TAG" -R "$REPO" --clobber "${FILES[@]}"

echo "Uploaded to draft $TAG. Next: build on the other OS if you haven't, then ./omni-release.sh --publish"
