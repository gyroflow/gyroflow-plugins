# Changelog

## Unreleased

- OpenFX: anamorphic clips are now stabilized to the part of the clip the timeline shows once de-squeezed (the SqueezeBorder region), not the whole de-squeezed frame. The auto zoom fits that window and the stabilized image is drawn into it, so a vertical 1.5x clip in a 2880x3840 timeline is no longer zoomed about 12% more than needed, and raising FOV crops right away. This restores the border-fitted zoom lost when the plugin moved to the upstream core.
- Fixed the zoom limit on 90/270-degree anamorphic footage: the core measured zoom against the recorded horizontal focal length, understating it by fx/fy (about 1.5x on a 1.5x vertical squeeze), so a 130% limit allowed close to 200%. The limit passed to the core is now scaled to the true zoom of the stabilized window, for either squeeze axis, and stays a limit (above the core's 50% "off" threshold). The Sony vertical sample test now sets the video speed, so it checks edges with real stabilization correction.

- Removed the cross-axis stretch disabling change after reported Resolve crashes, restoring the known-good independent controls. Restored the package version to 2.2.7 and kept the library/short name Gyroflow (Omni); only the long effect label includes the version. Future version changes require an explicit request.

- Fixed implicit anamorphic scaling on 90/270-degree footage after disabling stretch: the virtual output aspect now compensates the effective recorded-pixel focal ratio, while the host source/output rectangles retain their squeezed proportions. This shares the corrected geometry with rendering and zoom calculation. Added pixel-proportion regressions for both anamorphic axes at all quarter turns, with lens correction at 0% and 100%, alongside border-sampling tests with Disable stretch both on and off.

- Input vertical stretch now matches the horizontal preset dropdown and optional Custom slider. Effects saved with the slider-only control migrate their value on load, and selecting a preset preserves the custom value. OFX no longer enlarges the recorded source dimensions when Disable Gyroflow's stretch is checked: buffer fitting and zoom bounds use the actual recorded frame, avoiding sampling Resolve's side bars as image content. Added load-path and CPU pixel regressions for vertical/horizontal profiles and rotated buffers.

- Added Input vertical stretch (1x-2x), initialized from the lens profile's `input_vertical_stretch`. Either stretch axis enables anamorphic framing. The source-pixel crop guide now accounts for both axes and quarter-turn video rotation, fitting both width and height to the timeline aspect. Vertical edits are saved with the effect and separated in the stabilization cache.

- Renamed Input Squeeze ratio to Input horizontal stretch. Fresh lens/project loads initialize it from the lens profile's `input_horizontal_stretch`, selecting a preset or Custom within the supported 1x-2x range. Manual edits and saved effect values remain available; parameter IDs and crop-guide rendering behavior are unchanged.

- Lens correction now defaults to 0 for new instances and fresh project/preset loads, ignoring imported correction values and curves. Saved effect settings and subsequent user edits remain available.

- Added a Zooming selector before Integration method with Static zooming and Dynamic zooming (4-second smoothing). Static is the default; imported No zooming settings map to Static. Both modes work with anamorphic framing and participate in recalculation and instance isolation.
- Renamed the effect display name to `Gyroflow (Omni)` in OpenFX, Adobe, frei0r, and Shotcut, and the repository to `gyroflow-plugins-omni`. The OpenFX bundle folder is now `GyroflowOmni.ofx.bundle` on all platforms; the binary is named `GyroflowOmni.ofx` to match the bundle, as required for host discovery. Omni uses the distinct OpenFX identifier `xyz.gyroflow.omni` so it can coexist with the original Gyroflow plugin. Existing effects using `xyz.gyroflow` continue to refer to the original plugin.
- Added Nikon NRAW (`.NEV`) and Nikon R3D NE (`.R3D`) container metadata loading, including NRAW files renamed to `.R3D`, across the shared plugin loader and frei0r.
- Added `.NEV` to the file picker and Nikon metadata handling to footage matching and project rotation lookup. Existing BRAW timing uses the core path unchanged.
- Validated all three supplied Nikon ZR samples: 6060x3410 at 24000/1001 fps, 147/147/15 frames. The current parser extracts no IMU samples, so direct loading alone cannot provide gyro stabilization; RAW image decoding remains the editor's responsibility.
- Merged upstream `gyroflow/gyroflow-plugins` through `8a239ce` (2026-10-29), including core and dependency updates, preset persistence, instance isolation, and rotation fixes.
- Adapted anamorphic source framing to upstream's rotation-aware geometry and public core API. The squeeze border remains a guide; Zooming and FOV control the rendered crop.
- Separate stabilization cache entries by squeeze ratio so anamorphic settings cannot affect an instance using another ratio.

## [2.2.0] - 2026-08-09

### Added

- Added anamorphic input squeeze-ratio controls to the shared plugin parameter model used by the OpenFX and Adobe plugins.
- Added preset ratios from `1.0x` through `2.0x`, plus a custom `1.0x`–`2.0x` slider.
- Added the hidden `SqueezeBorder` metadata parameter, which publishes a centered source-pixel auto-crop guide as JSON.

### Changed

- Anamorphic mode preserves the raw image without applying lens-profile de-squeezing or changing the rendered output.
- Active squeeze ratios use the full source dimensions for the internal fit and avoid zooming in; the published border is a guide for host integrations and does not crop the image by itself.
