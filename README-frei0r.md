
# Gyroflow frei0r plugin

* Works with project files exported from [Gyroflow](http://gyroflow.xyz/)
* Allows you to apply the stabilization right in your frei0r-capable video editor

Some applications using frei0r, in which you can use this plugin:

- [MLT](https://www.mltframework.org/)
- [LiquidSoap](https://www.liquidsoap.info/)
- [Kdenlive](https://www.kdenlive.org/)
- [Shotcut](https://www.shotcut.org/)
- [FFMpeg](https://ffmpeg.org)
- [PureData](https://puredata.info/)
- [Open  Movie  Editor](http://www.openmovieeditor.org/)
- [Gephex](https://gephex.org/)
- [LiVES](http://lives.sf.net)
- [FreeJ](https://freej.dyne.org)
- [VeeJay](http://veejayhq.net)
- [Flowblade](https://jliljebl.github.io/flowblade/)

# Downloads

## https://github.com/gyroflow/gyroflow-frei0r/releases

# Installation

### Kdenlive:
1. Copy the plugin binary to `kdenlive/lib/frei0r-1/`
2. Copy [`frei0r_gyroflow.xml`](https://raw.githubusercontent.com/gyroflow/gyroflow-frei0r/main/frei0r_gyroflow.xml) to `kdenlive/bin/data/kdenlive/effects/`

# Usage

### FFmpeg:
1. Create a folder somewhere, copy the plugin binary to it, and set environment variable `FREI0R_PATH` to that dir. For example on Windows: `set FREI0R_PATH=C:\effects\`
2. Run ffmpeg: `ffmpeg -i input_video.mp4 -vf "frei0r=gyroflow:C_DRIVE_SEP_projects_DIR_SEP_my_project.gyroflow|0.5|n|0.001" result.mp4`
3. Parameters are: `project_file_path|smoothness|stabilization_overview|timestamp_scale`.
4. Because ffmpeg can't accept `:` or `/` in parameters, plugin will replace `_DRIVE_SEP_` with `:\` and `_DIR_SEP_` with `/`, so you can use parameter: `E_DRIVE_SEP_some_folder_DIR_SEP_my_project.gyroflow` for `E:\some_folder\my_project.gyroflow`


# Building from source
1. Get latest stable Rust language from: https://rustup.rs/
2. Clone the repo: `git clone https://github.com/gyroflow/gyroflow-frei0r.git`
3. Build the binary: `cargo build --release`
4. Resulting file will be in `target/release/` directory

<br>

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version 2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>