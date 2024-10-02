<p align="center">
  <h1 align="center">
    <a href="https://github.com/gyroflow/gyroflow-plugins#gh-light-mode-only">
      <img src="https://raw.githubusercontent.com/gyroflow/gyroflow/master/resources/logo_black.svg" alt="Gyroflow logo" height="100">
    </a>
    <a href="https://github.com/gyroflow/gyroflow-plugins#gh-dark-mode-only">
      <img src="https://raw.githubusercontent.com/gyroflow/gyroflow/master/resources/logo_white.svg" alt="Gyroflow logo" height="100">
    </a>
  </h1>

  <p align="center">
    Video stabilization using gyroscope data
    <br/>
    <br/>
    <a href="https://gyroflow.xyz">Homepage</a> •
    <a href="https://github.com/gyroflow/gyroflow-plugins/releases">Download</a> •
    <a href="https://docs.gyroflow.xyz">Documentation</a> •
    <a href="https://discord.gg/WfxZZXjpke">Discord</a> •
    <a href="https://github.com/gyroflow/gyroflow-plugins/issues">Report bug</a> •
    <a href="https://github.com/gyroflow/gyroflow-plugins/issues">Request feature</a>
  </p>
  <p align="center">    
    <a href="https://github.com/gyroflow/gyroflow-plugins/graphs/contributors">
      <img src="https://img.shields.io/github/contributors/gyroflow/gyroflow-plugins?color=dark-green" alt="Contributors">
    </a>
    <a href="https://github.com/gyroflow/gyroflow-plugins/issues/">
      <img src="https://img.shields.io/github/issues/gyroflow/gyroflow-plugins" alt="Issues">
    </a>
    <a href="https://github.com/gyroflow/gyroflow-plugins/blob/master/LICENSE">
      <img src="https://img.shields.io/github/license/gyroflow/gyroflow-plugins" alt="License">
    </a>
  </p>
</p>

# Gyroflow plugin for video editors
Gyroflow plugin for a video editor allows you to stabilize the video directly inside the video editor, where you work with the original video file, instead of rendering a stabilized video in the app and then importing it to the video editor.

If your camera has official lens-profiles and accurate gyro timing (GoPro 8+, Sony, Insta360, DJI), then you should be able to just apply the plugin to your clip. In the Adobe and Final Cut Pro plugin, it should load the gyro data automatically. In Resolve, you should click "Load for current file" or use the "Browse" button to select your video file or the `.gyroflow` project file.

Since Gyroflow supports a lot of different cameras and gyro sources, it's practically impossible to recreate all tools it offers (for example synchronization with optical flow) inside the plugin user interface.

If your camera requires synchronization (RED, RunCam, Hawkeye, phone apps, blackbox, etc.), the workflow starts inside the main Gyroflow application, where you load your video, lens profile, gyro data, you do all synchronization and parameters, but instead of rendering - you export a project file which includes all your parameters and gyro data. This can be easily done by using the `CTRL+S` shortcut, or using `Export -> Export project file (including gyro data)` in the application.

This exported project file is then loaded inside the Gyroflow plugin in the video editor and the plugin will process your pixels directly inside your editor according to the gyro data and all your parameters, but without any transcoding, recompression or additional processing.

This is especially important when working with RAW files (like BRAW or R3D), where you retain all your RAW controls like ISO, White Balance etc.

---

This repository contains the source code of [Gyroflow](https://github.com/gyroflow/gyroflow) video editor plugins. This includes OpenFX, Adobe and frei0r.<br>
Final Cut Pro plugin is hosted in an [external repository](https://github.com/latenitefilms/GyroflowToolbox/).

## Supported applications:
| Applications | Plugin type | Download | Nightly |
| ------------- | ------------- | ------------- | ------------- |
| <ul><li>[DaVinci Resolve](https://www.blackmagicdesign.com/products/davinciresolve)</li><li>[Assimilate SCRATCH](https://www.assimilateinc.com/products/)</li><li>[The Foundry Nuke](https://www.foundry.com/products/nuke-family/nuke)</li><li>[MAGIX Vegas](https://www.vegascreativesoftware.com/us/vegas-pro/)</li></ul> | **OpenFX** | [Download](https://github.com/gyroflow/gyroflow-plugins/releases) | [Windows](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-OpenFX-windows.zip)<br>[macOS](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-OpenFX-macos.zip)<br>[Linux](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-OpenFX-linux.zip) |
| <ul><li>[Adobe After Effects](https://www.adobe.com/products/aftereffects.html)</li><li>[Adobe Premiere](https://www.adobe.com/products/premiere.html)</li></ul> | **Adobe**  | [Download](https://github.com/gyroflow/gyroflow-plugins/releases) | [Windows](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-Adobe-windows.zip)<br>[macOS](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-Adobe-macos.zip) |
| <ul><li>[Final Cut Pro](https://www.apple.com/final-cut-pro/)</li></ul> | **FxPlug4**  | [Download](https://gyroflowtoolbox.io/) | --- |
| <ul><li>[Kdenlive](https://www.kdenlive.org/)</li><li>[Shotcut](https://www.shotcut.org/)</li><li>[FFmpeg](https://ffmpeg.org)</li><li>[MLT](https://www.mltframework.org/)</li><li>[LiquidSoap](https://www.liquidsoap.info/)</li><li>[PureData](https://puredata.info/)</li><li>[Open Movie Editor](http://www.openmovieeditor.org/)</li><li>[Gephex](https://gephex.org/)</li><li>[LiVES](http://lives.sf.net)</li><li>[FreeJ](https://freej.dyne.org)</li><li>[VeeJay](http://veejayhq.net)</li><li>[Flowblade](https://jliljebl.github.io/flowblade/)</li></ul> | **frei0r**  | [Download](https://github.com/gyroflow/gyroflow-plugins/releases) | [Windows](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-frei0r-windows.zip)<br>[macOS](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-frei0r-macos.zip)<br>[Linux](https://nightly.link/gyroflow/gyroflow-plugins/workflows/release/main/Gyroflow-frei0r-linux.zip) |

---

## Installation and usage
Gyroflow app includes a tool to install the plugins. Since version `v1.6.0` it has a dedicated **"Video editor plugins"** panel, which should show up (on bottom right) when Gyroflow detects you have Adobe or DaVinci Resolve installed. Using this panel is the easiest way to install and update the plugins.

The Final Cut Pro plugin, called [Gyroflow Toolbox](https://gyroflowtoolbox.io) is available on the Mac App Store as paid product (to cover support costs), however you can also build it from source [here](https://github.com/latenitefilms/GyroflowToolbox/).

For manual installation steps and more details, refer to [the documentation](https://docs.gyroflow.xyz/app/video-editor-plugins/general-plugin-workflow)

---

## License

Distributed under the GPLv3 License with App Store Exception. See [LICENSE](https://github.com/gyroflow/gyroflow-plugins/blob/main/LICENSE) for more information.
