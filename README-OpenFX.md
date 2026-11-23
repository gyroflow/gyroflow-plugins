<span class="badge-patreon"><a href="https://www.patreon.com/smartislav" title="Donate to this project using Patreon"><img src="https://img.shields.io/badge/patreon-donate-yellow.svg" alt="Patreon donate button" /></a></span>
![example workflow](https://github.com/yshui1/gyroflow-plugins-omni/actions/workflows/release.yml/badge.svg)

# Gyroflow (Omni) OpenFX plugin

The Resolve library name is **Gyroflow (Omni)**. The long effect label includes version **1.0.0** for hosts that use it in the effect header. Package versions are changed only when explicitly requested.

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

When you copy the effect to a different clip in Resolve, Gyroflow keeps the copied **Adjust parameters** values and Resolve keyframes when it switches to the destination clip. The destination supplies the video, gyro data, and timing.

## Zooming

**Zooming**, immediately before **Integration method** in **Adjust parameters**, offers
**Static zooming** (one crop for the clip) and **Dynamic zooming** (a crop that adapts
with a 4-second smoothing window). New instances default to Static zooming.
Loading a project or preset selects its zoom mode; No zooming maps to Static zooming.
Both choices also work with anamorphic squeeze ratios. The choice is saved with the
effect, and changing it recalculates stabilization without affecting copied instances.

## Input horizontal stretch

Use **Input horizontal stretch** in the plugin parameters to select a common anamorphic ratio or **Custom** for a value from `1.0x` to `2.0x`. Loading a lens profile or project initializes this control from the lens profile's `input_horizontal_stretch` within that range; values without a matching preset select **Custom**. You can adjust it afterward, and saved effects retain manual overrides. Internal parameter IDs remain unchanged for compatibility. The hidden `SqueezeBorder` parameter exposes the centered auto-crop region as JSON for host integrations. In Resolve's Edit and Color pages, stabilization and auto zoom fit that region to the timeline render, including after a project is reopened.

**Input vertical stretch** uses the same preset dropdown and optional Custom slider as horizontal stretch, initialized from the profile's `input_vertical_stretch` on fresh lens/project loads. Values saved by the earlier slider-only build migrate automatically; switching between presets preserves the custom value. Both axes refer to recorded pixels before video rotation; manual edits are saved with the effect. For the Sony vertical 1.5x setup, use horizontal 1.0x and vertical 1.5x, with the existing 270-degree rotations. Either stretch axis enables anamorphic framing.

The border guide is a centered rectangle in recorded source pixels. It accounts for both stretch axes and quarter-turn video rotation, cropping width or height to match the timeline after desqueeze. At 3240x3840 output, the border guide covers the full 3840x2160 recorded frame; stabilization may still zoom to cover motion. Apply desqueeze separately in Resolve. OFX always retains the recorded source dimensions when locating the clip in Resolve's buffers. It does not enlarge them by the profile stretch factors; this prevents side bars from being sampled as video. Quarter-turn processing also compensates the camera calibration's unequal focal axes in the internal output aspect, preserving squeezed pixel proportions. Resolve handles the final desqueeze.

### Export `.gyroflow` file in the Gyroflow app

Click the `Export project file (including gyro data)` in the Gyroflow app. You can also use `Ctrl+S` or `Command+S` shortcut

### Basic plugin usage

First you need to apply the plugin to the clip.
In DaVinci Resolve you can do that by going to the Fusion tab and inserting the "Warp -> Gyroflow (Omni)" after the media input node.
You can also apply the plugin on the Edit or Color page - it should work faster this way.

### Load the .gyroflow file

In DaVinci Resolve, go to the `Gyroflow (Omni)` plugin settings. Select the `.gyroflow` file in the `Project file` entry.
If your video file is from GoPro 8+, DJI or Insta360, you can also select video file directly. If it's from Sony or it's BRAW - you can also select the video file directly, but you need to load lens profile or preset after that.

### Zoom lens profiles

In the standalone app, choose **Lens profile → Create zoom profile** and select calibrations from the same lens and recording mode. Confirm the focal lengths and save the combined JSON. Resolve can open it through **Load preset/lens profile** or use a profile embedded in a Gyroflow project.

For automatic selection from a source video, put the JSON in `%LOCALAPPDATA%\Gyroflow\lens_profiles` on Windows. The clip must report the lens, recording mode, and focal lengths covered by the profile. For a fixed-focal-length clip, an exact calibration takes priority. The Inspector shows the range of the loaded zoom profile.

### Same-folder gyro files

Load the source clip with **Browse** or **Load for current file**. If the clip has no embedded motion,
the plugin looks alongside it for a matching `.gcsv` or `.csv`. The files must share
the clip's base name, such as `A001_007_20261119.MOV` and `A001_007_20261119.CSV`. CSV logs must use a
format Gyroflow understands, such as Sigma's `CAMERA IMU LOG`. Lowercase and uppercase extensions work.

For Sigma FP2 MOV clips, the plugin accepts a matching gyro sidecar as synced when its samples
start at zero and end between 1 ms before and 300 ms after the clip ends. The plugin
keeps the recorded sample timestamps.

### SIGMA fp CinemaDNG with embedded gyro data

SIGMA fp CinemaDNG takes recorded with fpSup's Gyro2 carry the gyro record in each frame's MakerNote.
Choose **Browse**, then select any DNG in the take, or use **Load for current file**. The plugin reads the
gyro, frame timing, readout time, and the lens's focal length and distortion from the DNG frames. It
doesn't need a sidecar file. A gap in the frame numbers splits the take, and the plugin loads the
consecutive run that contains the selected frame.

For a lens without electronic contacts, the frames have no lens data. Select a lens profile with
**Load preset/lens profile**, or use **Generic profile** (below). **Open in Gyroflow** saves a project that contains the parsed motion data
and opens it in the app, because the app can't read the DNG telemetry. The effect then uses that
project, so **Reload project** applies the edits you save in Gyroflow.

### Generic profile

For a lens without a profile, such as one without electronic contacts, open **Generic profile**, turn
on **Use generic profile**, and check **Lens focal (mm)**. The plugin then stabilizes with an
uncalibrated rectilinear lens of that focal length. This works for video clips, CinemaDNG takes, and
projects. When it's on, it replaces the clip's lens and any profile loaded with **Load preset/lens
profile**. Turn it off to go back to them.

- **Lens focal (mm)** and **Sensor width (mm)**: while the profile is off, the plugin fills these in
  from the clip's metadata when the clip has it (Sony clips and SIGMA fp CinemaDNG with an electronic
  lens). Your values stand while it's on.
- **Sensor width (mm):** the width of the sensor area the clip records. When the clip reports it, the
  clip's value is used. Otherwise set it yourself: 36 for full frame, about 23.5 for APS-C or Super 35.
- **Typical distortion for focal:** adds the barrel distortion typical of the focal length, from
  SIGMA fp profiles in Gyroflow's lens database. This suits RAW. For video the camera has already
  corrected for distortion (Sony's in-camera lens compensation, for example), so turn it off and leave
  **Corner distortion (%)** at 0. Negative values are barrel, positive values are pincushion.

The plugin also reads matching JSON clip metadata. For directly loaded clips, `frame_readout_time`
(in milliseconds) and `frame_readout_direction` supply the detected rolling shutter setting. You can
override the time under **Rolling Shutter**. Select the lens calibration with **Load preset/lens profile**.

If the Inspector reports **Not synced**, open the clip in Gyroflow, load the motion log there, sync
it, and export a `.gyroflow` project with gyro data. Loading that project in the plugin uses its
saved motion and sync points.

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
