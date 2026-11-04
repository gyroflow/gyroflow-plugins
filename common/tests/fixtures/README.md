# Video timing fixture

`video-timing.mp4` contains two synthetic black 16x16 frames at 24000/1001 fps.
It was generated with:

```text
ffmpeg -f lavfi -i color=c=black:s=16x16:r=24000/1001 -frames:v 2 -c:v libx264 -pix_fmt yuv420p -movflags +faststart video-timing.mp4
```

The metadata tests replace its `avc1` fourcc with `NRAW` or `NR3D` and mark the AVC configuration as a `free` box in memory
to test track parsing independently of decoding. It is not a Nikon RAW sample.
The opt-in Nikon sample test uses the original camera files separately.

`sony_aivascope_vertical_1_5x.json` is the supplied Sony ILCE-9M3 / Aivascope
vertical anamorphic calibration. It uses vertical stretch 1.5 and horizontal
stretch 1.0, with 3840x3240 calibration dimensions for 3840x2160 recorded pixels.
It covers profile initialization and rotated source-pixel border calculations.

The opt-in `sony_vertical_sample_cpu_edges` test uses `GYROFLOW_VERTICAL_SAMPLE`
to load the original C0999.MP4 gyro metadata with this profile. It renders five
sampled times on CPU using a white source inside black host pillarboxes, with
Dynamic zooming, FOV 1, a 130% zoom limit, and Disable stretch enabled. It checks
that the stabilized image does not sample the host's black side bars. It does
not decode the camera image or replace visual validation in Resolve.
