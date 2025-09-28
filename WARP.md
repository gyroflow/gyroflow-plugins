# WARP.md

This file provides guidance to WARP (warp.dev) when working with code in this repository.

# Gyroflow Video Editor Plugins

This repository contains video editor plugins that integrate Gyroflow's video stabilization capabilities directly into various video editing applications. The plugins support multiple formats: OpenFX (DaVinci Resolve, Nuke, Vegas), Adobe (After Effects, Premiere), and frei0r (Kdenlive, Shotcut, FFmpeg).

## Quick Start Commands

### Development Setup
```bash
# Install system dependencies (varies by platform)
just install-deps

# Update all Cargo dependencies
just update
```

### Building
```bash
# Build all plugins for deployment
just deploy

# Build specific plugin types
just adobe deploy    # Adobe plugins (After Effects, Premiere)
just ofx deploy      # OpenFX plugins (DaVinci Resolve, Nuke, etc.)
just frei0r deploy   # frei0r plugins (Kdenlive, Shotcut, etc.)

# Development builds (faster, with debug info)
cargo build --release
```

### Testing Individual Plugins
```bash
# Test Adobe plugin (macOS)
just adobe release

# Test OpenFX plugin (macOS) 
just ofx release

# Build and check compilation
cargo check --workspace
```

## Architecture Overview

The project follows a modular plugin architecture with a shared core:

```
┌─────────────────────────────────────────────────────────────┐
│                    Video Editor Applications                │
├─────────────────┬─────────────────┬─────────────────────────┤
│   Adobe Suite   │   OpenFX Apps   │   frei0r Applications   │
│ (AE, Premiere)  │ (Resolve, Nuke) │ (Kdenlive, Shotcut)     │
├─────────────────┼─────────────────┼─────────────────────────┤
│   adobe/        │   openfx/       │       frei0r/           │
│   ├─ lib.rs     │   ├─ lib.rs     │       ├─ lib.rs        │
│   ├─ ui.rs      │   ├─ gyroflow.rs│       └─ frei0r.rs     │
│   └─ premiere.rs│   └─ fuscript.rs│                         │
├─────────────────┴─────────────────┴─────────────────────────┤
│                    common/ (shared base)                    │
│  ┌─ GyroflowPluginBase - Core plugin functionality         │
│  ├─ Stabilization management & GPU context                 │
│  ├─ Parameter handling & serialization                     │
│  └─ Gyroflow core integration                              │
├─────────────────────────────────────────────────────────────┤
│              gyroflow-core (external crate)                │
│  ┌─ StabilizationManager - Video processing pipeline       │
│  ├─ GPU acceleration (OpenCL, Metal, WGPU)                │
│  ├─ Lens correction & motion data                          │
│  └─ Project file (.gyroflow) parsing                      │
└─────────────────────────────────────────────────────────────┘
```

### Key Components

**Plugin Interfaces:**
- `adobe/` - Adobe After Effects and Premiere Pro plugin using AE SDK bindings
- `openfx/` - OpenFX standard plugin for DaVinci Resolve, Nuke, Vegas Pro
- `frei0r/` - frei0r filter plugin for open-source video editors

**Shared Foundation:**
- `common/` - `GyroflowPluginBase` provides unified parameter handling, GPU context management, and stabilization caching
- External `gyroflow-core` crate handles the actual video stabilization processing

**Parameter System:**
The plugins expose a consistent set of parameters across all formats:
- Project data and file paths
- Stabilization settings (FOV, smoothness, lens correction)
- Output configuration (size, rotation, horizon lock)
- Keyframe integration with host applications

## Development Environment

### Platform Requirements
- **Rust**: Latest stable toolchain (install via [rustup](https://rustup.rs/))
- **Just**: Build runner (install via `cargo install just`)
- **Platform SDKs**:
  - macOS: Xcode command line tools
  - Windows: Visual Studio Build Tools
  - Linux: GCC/Clang, pkg-config

### External Dependencies
The build system automatically downloads and manages:
- **Adobe SDK**: After Effects and Premiere Pro headers
- **OpenCL**: Cross-platform GPU compute library  
- **LLVM/Clang**: For C++ interop and bindgen
- **Vulkan SDK**: GPU graphics API (via GitHub Actions)

### Rust Workspace Structure
```toml
# Root workspace coordinates 4 crates:
[workspace]
members = ["common", "adobe", "openfx", "frei0r"]

# Each plugin crate depends on common:
[dependencies]
gyroflow-plugin-base = { path = "../common" }
```

## Common Development Tasks

### Plugin Installation Locations
```bash
# macOS
/Library/OFX/Plugins/                    # OpenFX
/Library/Application Support/Adobe/      # Adobe
/usr/local/lib/frei0r-1/                 # frei0r

# Windows  
C:\Program Files\Common Files\OFX\Plugins\     # OpenFX
C:\Program Files\Adobe\Common\Plug-ins\        # Adobe

# Linux
/usr/OFX/Plugins/                        # OpenFX
/usr/lib/frei0r-1/                       # frei0r
```

### GPU Acceleration
Plugins support multiple GPU backends:
- **Metal** (macOS): Native Apple GPU acceleration
- **OpenCL**: Cross-platform compute (Intel, AMD, NVIDIA)
- **WGPU**: Modern graphics API abstraction

GPU context initialization is handled in `common/src/lib.rs`:
```rust path=/Users/jacobychye/gyroflow-plugins/common/src/lib.rs start=97
pub fn initialize_gpu_context(&mut self) {
    if !self.context_initialized {
        gyroflow_core::gpu::initialize_contexts();
        self.context_initialized = true;
    }
}
```

### Plugin Parameter Management
All plugins share a unified parameter system defined in `Params` enum:
```rust path=/Users/jacobychye/gyroflow-plugins/common/src/lib.rs start=22
#[derive(Debug, Copy, Clone, Hash, PartialEq, PartialOrd, Eq, Ord, serde::Serialize, serde::Deserialize)]
pub enum Params {
    ProjectData,
    ProjectPath, 
    Fov,
    Smoothness,
    // ... additional parameters
}
```

### Caching Strategy
Plugins implement an LRU cache for stabilization managers to optimize performance when the same clip is used multiple times:
```rust path=/Users/jacobychye/gyroflow-plugins/common/src/lib.rs start=83
pub manager_cache: Mutex<LruCache<String, Arc<StabilizationManager>>>,
```

## Troubleshooting

### Build Issues
- **Missing Adobe SDK**: Run `just install-deps` to download required headers
- **OpenCL not found**: Install platform-appropriate OpenCL development packages
- **Rust toolchain errors**: Update with `rustup update stable`

### Runtime Issues
- **Plugin not visible**: Check installation paths match platform conventions
- **GPU errors**: Verify OpenCL/Metal drivers and fallback to CPU processing
- **Project file errors**: Ensure `.gyroflow` files are exported from main Gyroflow application

### Debugging
Log files are written to:
- **macOS/Linux**: `~/.local/share/gyroflow/gyroflow-{plugin}.log`
- **Windows**: `%APPDATA%\gyroflow\gyroflow-{plugin}.log`

Enable detailed logging by building with debug symbols:
```bash
cargo build --release
# Or for maximum debug info:
RUSTFLAGS="-C debug-assertions=on" cargo build --release
```

## Release Process

The project uses GitHub Actions for automated builds:
1. **Dependency Installation**: Platform-specific SDK and library setup
2. **Multi-target Compilation**: Windows, macOS (Universal), Linux builds  
3. **Code Signing**: macOS plugins are notarized for distribution
4. **Artifact Generation**: ZIP files and DMG installers created
5. **Release Publishing**: Automated GitHub releases for tagged versions

Manual release preparation:
```bash
just publish "2.1.2"  # Updates version numbers and creates git tag
```

## Working with Gyroflow Core

The plugins integrate with the main Gyroflow stabilization engine via the `gyroflow-core` crate:

```rust path=null start=null
// Key integration points:
use gyroflow_core::{ StabilizationManager, keyframes::*, stabilization::* };

// Manager creation and caching
let stab = Arc::new(StabilizationManager::default());
stab.load_project_data(&project_data)?;
stab.set_output_size(width, height);
```

The core handles:
- **Project File Parsing**: `.gyroflow` files with stabilization parameters
- **Motion Data Processing**: Gyroscope, accelerometer, and optical flow data
- **Lens Correction**: Distortion profiles and field-of-view adjustments  
- **Stabilization Pipeline**: Multi-threaded video processing with GPU acceleration

This architecture allows the plugins to focus on host application integration while leveraging the full power of Gyroflow's stabilization algorithms.