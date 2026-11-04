<span class="badge-patreon"><a href="https://www.patreon.com/smartislav" title="Donate to this project using Patreon"><img src="https://img.shields.io/badge/patreon-donate-yellow.svg" alt="Patreon donate button" /></a></span>
![example workflow](https://github.com/yshui1/gyroflow-plugins-omni/actions/workflows/release.yml/badge.svg)

# Gyroflow (Omni) OpenFX plugin

* Works with stabilization data exported with [gyroflow](http://gyroflow.xyz/)
* Allows you to apply the stabilization right in your OpenFX-capable video editor

# Installation

Grab the archive for your OS from the [releases page](https://github.com/yshui1/gyroflow-plugins-omni/releases).

## Linux

    mkdir -p /usr/OFX/Plugins
    cd /usr/OFX/Plugins
    sudo unzip ${PATH_TO}/gyroflow-ofx-linux.zip

## MacOS

Copy the `GyroflowOmni.ofx.bundle` from the archive into the `/Library/OFX/Plugins` directory.
Create the directory if it doesn't exist yet.
Then in Resolve, make sure to go to Preferences -> Video plugins and enable GyroflowOmni.ofx.bundle.

## Windows

Copy the `GyroflowOmni.ofx.bundle` from the archive into the `C:\Program Files\Common Files\OFX\Plugins` folder.
Create the folder if it doesn't exist yet.

## For more detailed instructions, see the [docs](https://docs.gyroflow.xyz/app/video-editor-plugins/davinci-resolve-openfx#installation)

The bundle binary must be named `GyroflowOmni.ofx` (for example, `Contents/Win64/GyroflowOmni.ofx` on Windows). Omni uses the distinct plugin ID `xyz.gyroflow.omni`, so it can coexist with the original Gyroflow plugin. Existing instances of the original plugin remain attached to it; add **Gyroflow (Omni)** as a new effect to use this version.

# Usage

Lens correction defaults to **0** for new instances and fresh project/preset loads, including projects with lens-correction keyframes. You can adjust or keyframe it afterward; saved effects retain their settings.

## Zooming

**Zooming**, immediately before **Integration method** in **Adjust parameters**, offers
**Static zooming** (one crop for the clip) and **Dynamic zooming** (a crop that adapts
with a 4-second smoothing window). New instances default to Static zooming.
Loading a project or preset selects its zoom mode; No zooming maps to Static zooming.
Both choices also work with anamorphic squeeze ratios. The choice is saved with the
effect, and changing it recalculates stabilization without affecting copied instances.

## Input horizontal stretch

Use **Input horizontal stretch** in the plugin parameters to select a common anamorphic ratio or **Custom** for a value from `1.0x` to `2.0x`. Loading a lens profile or project initializes this control from the lens profile's `input_horizontal_stretch` within that range; values without a matching preset select **Custom**. You can adjust it afterward, and saved effects retain manual overrides. Internal parameter IDs remain unchanged for compatibility. In anamorphic mode, the plugin preserves the raw image and exposes the centered auto-crop region as JSON through the hidden `SqueezeBorder` parameter. This border is a guide for host integrations; it does not crop or de-squeeze the rendered image.

**Input vertical stretch** uses the same preset dropdown and optional Custom slider as horizontal stretch, initialized from the profile's `input_vertical_stretch` on fresh lens/project loads. Values saved by the earlier slider-only build migrate automatically; switching between presets preserves the custom value. Both axes refer to recorded pixels before video rotation; manual edits are saved with the effect. For the Sony vertical 1.5x setup, use horizontal 1.0x and vertical 1.5x, with the existing 270-degree rotations. Either stretch axis enables anamorphic framing.

The border guide is a centered rectangle in recorded source pixels. It accounts for both stretch axes and quarter-turn video rotation, cropping width or height to match the timeline after desqueeze. At 3240x3840 output, the border guide covers the full 3840x2160 recorded frame; stabilization may still zoom to cover motion. Apply desqueeze separately in Resolve. With **Disable Gyroflow's stretch** checked, OFX retains the recorded source dimensions when locating the clip in Resolve's buffers. It does not enlarge them by the profile stretch factors; this prevents side bars from being sampled as video. Quarter-turn processing also compensates the camera calibration's unequal focal axes in the internal output aspect, preserving squeezed pixel proportions. Resolve handles the final desqueeze.

### Export `.gyroflow` file in the Gyroflow app

Click the `Export project file (including gyro data)` in the Gyroflow app. You can also use `Ctrl+S` or `Command+S` shortcut

### Basic plugin usage

First you need to apply the plugin to the clip.
In DaVinci Resolve you can do that by going to the Fusion tab and inserting the "Warp -> Gyroflow (Omni)" after the media input node.
You can also apply the plugin on the Edit or Color page - it should work faster this way.

### Load the .gyroflow file

In DaVinci Resolve, go to the `Gyroflow (Omni)` plugin settings. Select the `.gyroflow` file in the `Project file` entry.
If your video file is from GoPro 8+, DJI or Insta360, you can also select video file directly. If it's from Sony or it's BRAW - you can also select the video file directly, but you need to load lens profile or preset after that.

### Nikon NRAW and R3D NE

Load Nikon `.NEV` (NRAW) or `.R3D` (R3D NE) clips with **Browse** or **Load for current file**.
The plugin reads the Nikon QuickTime container's dimensions, frame rate, duration, and camera metadata.
NRAW files already renamed to `.R3D` also load; renaming is unnecessary and does not convert the codec.
This metadata path is for Nikon R3D NE, not RED cameras' native R3D container.

Your editor must support decoding the recording format. The plugin processes the editor's decoded frames,
retaining the editor's RAW color controls; it does not add a standalone RAW decoder.

The current telemetry parser extracts **no IMU samples** from the supplied ZR clips (`DSC_0073.NEV`,
its renamed `.R3D` copy, and native `DSC_0003.R3D`). Loading these files therefore does not provide
gyro stabilization by itself. Use a suitable lens profile and synchronized external motion data,
export a `.gyroflow` project **including gyro data**, and load that project in the plugin.
The Inspector reports missing gyro data when loading a clip without usable motion samples.

To repeat the camera-file validation in PowerShell:

```powershell
$env:GYROFLOW_NIKON_SAMPLE_DIR = 'C:\Raw Video\Gyroflow NRAW'
cargo test --manifest-path common/Cargo.toml --lib nikon_zr_samples -- --ignored --nocapture
```

All three files were verified at 6060x3410 and 24000/1001 fps. The two `DSC_0073` files have
147 frames (6.131125 seconds); `DSC_0003.R3D` has 15 frames (0.625625 seconds).
These checks validate metadata loading, not an editor render or RAW decoding.

## For more detailed instructions, see the [docs](https://docs.gyroflow.xyz/app/video-editor-plugins/general-plugin-workflow)


# License

This software is licensed under GNU General Public License version 3 ([LICENSE](LICENSE))

# Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the GNU General Public License version 3, shall be
licensed as above, without any additional terms or conditions.
