# Fix for DaVinci Resolve Aspect Ratio Stretching Issue

## Problem Description

The Gyroflow OpenFX plugin was causing video stretching when used in DaVinci Resolve projects with aspect ratios that differ from the original video file. This occurred when:

1. The project timeline has a different aspect ratio than the source video (e.g., 16:9 video in a 21:9 project)
2. DaVinci Resolve automatically scales/crops the input to match the project aspect ratio
3. The Gyroflow plugin was applying the original video's aspect ratio calculations to the already-transformed source image

## Root Cause Analysis

The issue was in two main areas:

### 1. Input Rectangle Calculation (`openfx/src/gyroflow.rs`, line 280)
The plugin was using the original video's aspect ratio to calculate the source rectangle:
```rust
let src_rect = GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio);
```

This caused problems when DaVinci Resolve had already transformed the source image to match the project aspect ratio.

### 2. Output Size Initialization (`common/src/lib.rs`, line 580)
The stabilization manager was always defaulting to the timeline output size without considering aspect ratio mismatches between the video and project.

## Solution Implemented

### 1. Smart Source Rectangle Detection
Modified the OpenFX plugin to detect when the source aspect ratio differs from the original video aspect ratio:

```rust
// Use source image aspect ratio instead of original video aspect ratio to prevent stretching
// This is especially important in DaVinci Resolve where the project aspect ratio differs from video aspect ratio
let src_aspect_ratio = src_size.0 as f64 / src_size.1 as f64;
let src_rect = if (src_aspect_ratio - org_ratio).abs() > 0.01 {
    // If the source aspect ratio differs significantly from the original, use source aspect ratio
    // This happens when DaVinci Resolve crops/scales the input to match project aspect ratio
    (0, 0, src_size.0, src_size.1)
} else {
    // Use original video aspect ratio when source matches original (normal case)
    GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio)
};
```

### 2. Intelligent Output Size Selection
Enhanced the stabilization manager to automatically detect aspect ratio mismatches and choose the appropriate output dimensions:

```rust
if out_size != (0, 0) {
    // Check if the output size aspect ratio differs significantly from video aspect ratio
    let video_aspect = md.width as f64 / md.height as f64;
    let output_aspect = out_size.0 as f64 / out_size.1 as f64;
    if (video_aspect - output_aspect).abs() > 0.01 {
        // Timeline has different aspect ratio - use timeline dimensions
        log::info!("Using timeline aspect ratio {:.3} instead of video aspect ratio {:.3}", output_aspect, video_aspect);
        stab.params.write().output_size = out_size;
    } else {
        // Aspect ratios match - use video dimensions
        stab.params.write().output_size = (md.width as usize, md.height as usize);
    }
}
```

### 3. Debug Logging
Added comprehensive logging to help diagnose aspect ratio issues:

```rust
log::debug!("Aspect ratio handling: org_ratio={:.3}, src_aspect={:.3}, out_aspect={:.3}, src_size={:?}, out_size={:?}, src_rect={:?}, out_rect={:?}", 
           org_ratio, src_aspect_ratio, out_size.0 as f64 / out_size.1 as f64, src_size, out_size, src_rect, out_rect);
```

## Technical Details

### Detection Threshold
The solution uses a threshold of 0.01 for aspect ratio comparisons to account for floating-point precision while still detecting meaningful differences.

### Compatibility
This fix maintains backward compatibility with:
- Standard workflows where video and project aspect ratios match
- Other OpenFX hosts (Nuke, Vegas, etc.)
- Existing stabilization projects

### Performance Impact
The fix adds minimal computational overhead - just a few floating-point comparisons and aspect ratio calculations.

## Testing Recommendations

To test the fix:

1. **Build the plugin**: Run `just ofx deploy` to build the OpenFX plugin
2. **DaVinci Resolve test cases**:
   - 16:9 video in 21:9 timeline
   - 4:3 video in 16:9 timeline
   - Square video in widescreen timeline
3. **Verify no regression**: Test standard cases where aspect ratios match
4. **Check logs**: Enable debug logging and verify aspect ratio calculations are correct

## Files Modified

1. `openfx/src/gyroflow.rs` - Fixed source rectangle calculation logic
2. `common/src/lib.rs` - Enhanced output size selection and added logging

The fix is targeted specifically at the aspect ratio handling issue while preserving all existing functionality and stability of the Gyroflow plugin.