# Video timing fixture

`video-timing.mp4` contains two synthetic black 16x16 frames at 24000/1001 fps.
It was generated with:

```text
ffmpeg -f lavfi -i color=c=black:s=16x16:r=24000/1001 -frames:v 2 -c:v libx264 -pix_fmt yuv420p -movflags +faststart video-timing.mp4
```

The metadata tests replace its `avc1` fourcc with `NRAW` or `NR3D` and mark the AVC configuration as a `free` box in memory
to test track parsing independently of decoding. It is not a Nikon RAW sample.
The opt-in Nikon sample test uses the original camera files separately.
