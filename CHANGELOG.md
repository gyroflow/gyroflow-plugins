# Changelog

## Unreleased

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
