# Changelog

## [2.2.0] - 2026-08-09

### Added

- Added anamorphic input squeeze-ratio controls to the shared plugin parameter model used by the OpenFX and Adobe plugins.
- Added preset ratios from `1.0x` through `2.0x`, plus a custom `1.0x`–`2.0x` slider.
- Added the hidden `SqueezeBorder` metadata parameter, which publishes a centered source-pixel auto-crop guide as JSON.

### Changed

- Anamorphic mode preserves the raw image without applying lens-profile de-squeezing or changing the rendered output.
- Active squeeze ratios use the full source dimensions for the internal fit and avoid zooming in; the published border is a guide for host integrations and does not crop the image by itself.
