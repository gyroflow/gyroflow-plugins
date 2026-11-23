# Releasing Gyroflow (Omni) plugins

Omni has no CI. Each release is built by hand: the Windows files on a Windows PC and the macOS files on a Mac. Both go into one **draft** GitHub release, which you publish once it has every file.

The Gyroflow (Omni) app installs and updates plugins from the **latest published release** of [`yshui1/gyroflow-plugins-omni`](https://github.com/yshui1/gyroflow-plugins-omni/releases). The app never sees drafts, so nobody is offered a release that's missing their OS.

The app's releases work the same way. See `RELEASING.md` in the `gyroflow-omni` repo.

## What a release contains

| File | Built on | Used by |
| --- | --- | --- |
| `Gyroflow-OpenFX-windows.zip` | Windows | the app's OpenFX install button, manual installs |
| `Gyroflow-Adobe-windows.aex` | Windows | the app's Adobe install button |
| `Gyroflow-OpenFX-macos.zip` | Mac | the app's OpenFX install button |
| `Gyroflow-Adobe-macos.zip` | Mac | the app's Adobe install button |
| `Gyroflow-OpenFX-macos.dmg`, `Gyroflow-Adobe-macos.dmg` | Mac | manual installs (optional) |

The file names are what the app downloads (`src/nle_plugins.rs` in `gyroflow-omni`), so don't rename them. Publishing is blocked until the four required files are in the draft.

## One-time setup

### Windows

- Rust, LLVM (`C:\Program Files\LLVM`), 7-Zip and the [GitHub CLI](https://cli.github.com) (`gh auth login`)
- `just`: `winget install Casey.Just`

### Mac

- Xcode, Rust, `gh` (`gh auth login`) and `just`: `brew install just`
- Signing and notarization, so macOS opens the plugins without warnings:
  1. Install your **Developer ID Application** certificate in Keychain. It's under Certificates in your Apple Developer account.
  2. The scripts use it automatically when it's the only Developer ID Application certificate in Keychain. If you have several, find the hash (`security find-identity -v -p codesigning`) and run the script with `SIGNING_FINGERPRINT=<hash>`.
  3. Save your notarization login. It asks for an app-specific password, which you create at [account.apple.com](https://account.apple.com):

     ```bash
     xcrun notarytool store-credentials omni-notary --apple-id <your Apple ID> --team-id <your team ID>
     ```

     The profile stays in your login Keychain, so every later release (of the app and the plugins) reuses it. You only redo this on a new Mac or after revoking the app-specific password. To check it, run `xcrun notarytool history --keychain-profile omni-notary`: it should print "Successfully received submission history". The scripts run the same check before building. To use a profile with another name, set `NOTARY_PROFILE=<name>`.

## Releasing a version

### 1. Set the version

Every plugin crate has the same version: `openfx`, `adobe` and `frei0r` (`Cargo.toml` and `Cargo.lock`). The scripts refuse to run if they differ. Update the version line in `README-OpenFX.md` and add a `CHANGELOG.md` entry, then commit and push. The release is tagged at the commit you're on, so it has to be on GitHub.

Write the release notes to a Markdown file, e.g. `notes.md`. The app shows them in its update popup. Without `-NotesFile` / `--notes`, GitHub generates notes from the commit list.

### 2. Build on Windows

```powershell
.\omni-release.ps1 -NotesFile notes.md
```

This runs the `common` tests, builds OpenFX and Adobe (`just ofx deploy`, `just adobe deploy`) with `OMNI_RELEASE=1`, and checks that `GyroflowOmni.ofx` reports `<version>.0`. It then creates the draft `v<version>` and uploads the Windows files.

### 3. Build on the Mac

Pull the same commit, then:

```bash
./omni-release.sh
```

This builds both architectures, signs with your Developer ID, notarizes the zips and dmgs, staples the dmgs and uploads them to the same draft. Notarization usually takes a few minutes per file. If Apple rejects a file, the script prints the submission and the command that shows Apple's log.

Either OS can go first. Whichever runs first creates the draft, and the other adds to it. Running a script again replaces its files in the draft.

### 4. Check and publish

Install the plugins from the draft's files and try them in Resolve or Premiere on each OS. Then run either of these:

```powershell
.\omni-release.ps1 -Publish
```

```bash
./omni-release.sh --publish
```

Publishing checks that every required file is in the draft, asks you to confirm, then publishes the release as the latest. From then on, the app's Video editor plugins section offers it.

## Script options

| Windows | Mac | Does |
| --- | --- | --- |
| `-NotesFile <file>` | `--notes <file>` | Release notes (also replaces the notes of an existing draft) |
| `-DryRun` | `--dry-run` | Build and check, but don't touch GitHub. On the Mac it still notarizes. |
| `-SkipBuild` | `--skip-build` | Upload the files already in `target/` |
| `-Publish` | `--publish` | Publish the draft instead of building |
| | `--adhoc` | Allow a build without a Developer ID (test builds only; browser downloads then need `xattr -dr com.apple.quarantine <bundle>`) |

## Version numbers

- `OMNI_RELEASE=1` (set by the scripts) gives release version numbers: `1.0.0.0` in the Windows file version and the macOS bundle version. The app shows these as `1.0.0` and marks them up to date when they match the latest release.
- Other local builds report `1.0.0-dev`. The app shows them in red with an Update button.
- The OpenFX descriptor version (`PluginVersion(1, 2)` in `openfx/src/gyroflow.rs`) and identifier `xyz.gyroflow.omni` don't follow the release version. Leave them as they are, so saved Resolve projects keep finding the effect.

## Troubleshooting

- **"isn't pushed to origin" / "uncommitted changes":** commit and push first. Untracked files are ignored.
- **"is already published":** published releases are never overwritten. Bump the version for a fix.
- **Wrong version reported:** a stale build was reused. Run again without `-SkipBuild`.
- **The GitHub Actions workflow** (`.github/workflows/release.yml`) is upstream's and only runs when started by hand. It needs upstream's signing secrets, so it isn't part of this process.
