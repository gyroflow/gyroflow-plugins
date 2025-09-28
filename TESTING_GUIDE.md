# Testing Guide: Gyroflow Aspect Ratio Fix in DaVinci Resolve

## ✅ Installation Complete

The Gyroflow OpenFX plugin with the aspect ratio fix has been successfully built and installed to:
```
/Library/OFX/Plugins/Gyroflow.ofx.bundle/
```

## 🧪 Testing Steps

### 1. Enable the Plugin in DaVinci Resolve

1. Open **DaVinci Resolve**
2. Go to **DaVinci Resolve > Preferences**
3. Navigate to **Video Plugins**
4. Find **"Gyroflow.ofx.bundle"** in the list
5. Make sure it's **checked/enabled**
6. Click **Save** and restart DaVinci Resolve

### 2. Test Scenarios

#### Scenario A: 16:9 Video in 21:9 Timeline (Most Common Issue)
1. Create a new project with **21:9 aspect ratio** (e.g., 2560x1080)
2. Import a **16:9 video** (e.g., 1920x1080)
3. Add the video to the timeline
4. Go to **Fusion** tab
5. Add **Gyroflow** effect from **Tools > Effects Library > OFX Plugins > Gyroflow**
6. Load a gyroflow project file or select your video file
7. **Expected Result**: Video should maintain proper proportions without stretching

#### Scenario B: 4:3 Video in 16:9 Timeline
1. Create a new project with **16:9 aspect ratio** (e.g., 1920x1080)
2. Import a **4:3 video** (e.g., 1440x1080)
3. Follow steps 3-7 from Scenario A
4. **Expected Result**: Video should maintain proper 4:3 proportions

#### Scenario C: Square Video in Widescreen Timeline
1. Create a new project with **16:9 aspect ratio**
2. Import a **square video** (e.g., 1080x1080)
3. Follow steps 3-7 from Scenario A
4. **Expected Result**: Video should remain square without stretching

### 3. Verification Points

#### ✅ What Should Work (Fixed Issues)
- [ ] Video maintains proper aspect ratio when project and video ratios differ
- [ ] No horizontal/vertical stretching occurs
- [ ] Stabilization works correctly
- [ ] Video quality is preserved

#### 🔍 What to Check
- [ ] Video doesn't appear stretched or squeezed
- [ ] Circular objects (like people's faces, wheels, etc.) remain circular
- [ ] Text/logos maintain proper proportions
- [ ] Stabilization effectiveness is preserved

### 4. Debugging Information

#### View Logs
The plugin generates detailed logs for debugging:
```bash
tail -f ~/.local/share/gyroflow/gyroflow-ofx.log
```

Look for these log messages indicating the fix is working:
- `"Using timeline aspect ratio X.XXX instead of video aspect ratio Y.YYY"`
- `"Aspect ratio handling: org_ratio=X.XXX, src_aspect=Y.YYY, out_aspect=Z.ZZZ"`

#### If Issues Persist
1. Check that you're using the **Fusion tab** (not Color or Edit page)
2. Try the **"Don't draw outside"** parameter if content appears cropped
3. Verify the gyroflow project file was created with matching video dimensions
4. Restart DaVinci Resolve if the plugin doesn't appear

### 5. Performance Verification

The fix should not impact performance:
- [ ] Rendering speed is similar to before
- [ ] Preview playback is smooth
- [ ] Memory usage is normal

### 6. Backward Compatibility Test

Test that normal cases still work:
- [ ] Same aspect ratio projects (16:9 video in 16:9 timeline) work normally
- [ ] Adobe After Effects (if available) still works correctly
- [ ] Other OpenFX hosts (Nuke, etc.) are unaffected

## 📝 Reporting Results

If you encounter any issues, please note:

1. **Project Settings**: Timeline resolution and aspect ratio
2. **Source Video**: Resolution and aspect ratio
3. **Expected vs Actual**: What you expected to see vs what happened
4. **Log Excerpts**: Any relevant entries from the log file
5. **DaVinci Resolve Version**: Help > About DaVinci Resolve

## 🎯 Success Criteria

The fix is working correctly when:
- ✅ Video maintains proper aspect ratio across different project/video combinations
- ✅ No stretching occurs in any dimension
- ✅ Stabilization quality is preserved
- ✅ Performance is unaffected
- ✅ Backward compatibility is maintained

## 🔄 If You Need to Rebuild

If you need to make changes and rebuild:
```bash
cd /Users/jacobychye/gyroflow-plugins
source "$HOME/.cargo/env"
just ofx deploy
# Then manually copy the plugin as done before
```

The updated plugin with your aspect ratio fix is ready for testing! 🚀