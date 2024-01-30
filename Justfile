set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

ExtDir := justfile_directory() / "ext"
export AESDK_ROOT := ExtDir / "AfterEffects"

# ――――――――――――――――――――――――――――――――――――――― MacOS ―――――――――――――――――――――――――――――――――――――――――
export DYLD_FALLBACK_LIBRARY_PATH := if os() == "macos" { if path_exists(`xcode-select --print-path` + "/Toolchains/XcodeDefault.xctoolchain/usr/lib/") == "true" { `xcode-select --print-path` + "/Toolchains/XcodeDefault.xctoolchain/usr/lib/" } else { `xcode-select --print-path` + "/usr/lib/" } } else { "" }
export MACOSX_DEPLOYMENT_TARGET := "10.15"
# ――――――――――――――――――――――――――――――――――――――― MacOS ―――――――――――――――――――――――――――――――――――――――――

# ――――――――――――――――――――――――――――――――――――――― Clang ―――――――――――――――――――――――――――――――――――――――――
export LIBCLANG_PATH := if os() == "macos" { DYLD_FALLBACK_LIBRARY_PATH } else { if path_exists(ExtDir / "llvm/bin") == "true" { ExtDir / "llvm/bin" } else { env_var_or_default("LIBCLANG_PATH", if path_exists("/usr/lib/llvm-13/lib/") == "true" { "/usr/lib/llvm-13/lib/" } else { "" }) } }
LLVMPath := LIBCLANG_PATH
# ――――――――――――――――――――――――――――――――――――――― Clang ―――――――――――――――――――――――――――――――――――――――――

export PATH := LLVMPath + (if os() == "windows" { ";" } else { ":" }) + env_var('PATH')

adobe *param:
    just -f adobe/Justfile {{param}}

ofx *param:
    just -f openfx/Justfile {{param}}

frei0r *param:
    just -f frei0r/Justfile {{param}}

deploy:
    just -f adobe/Justfile deploy
    just -f openfx/Justfile deploy
    just -f frei0r/Justfile deploy

publish version:
    #!/bin/bash
    git clone --depth 1 git@github.com:gyroflow/gyroflow-plugins.git __publish
    pushd __publish
    sed -i'' -E "0,/version = \"[0-9\.a-z-]+\"/s//version = \"{{version}}\"/" adobe/Cargo.toml
    sed -i'' -E "0,/version = \"[0-9\.a-z-]+\"/s//version = \"{{version}}\"/" openfx/Cargo.toml
    sed -i'' -E "0,/version = \"[0-9\.a-z-]+\"/s//version = \"{{version}}\"/" frei0r/Cargo.toml
    git commit -a -m "Release v{{version}}"
    git tag -a "v{{version}}" -m "Release v{{version}}"
    git push origin
    git push origin "v{{version}}"
    popd
    rm -rf __publish
    git pull

# ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
# ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ Dependencies ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
# ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

[windows]
install-deps:
    #!powershell
    $ProgressPreference = 'SilentlyContinue'
    $ErrorActionPreference = 'Stop'

    mkdir "{{ExtDir}}" -ErrorAction SilentlyContinue
    cd {{ExtDir}}

    # OpenCL
    if (-not (Test-Path -Path "./vcpkg/installed/x64-windows-release/lib/OpenCL.lib")) {
        rm -Recurse -Force .\vcpkg -ErrorAction SilentlyContinue
        git clone --depth 1 https://github.com/Microsoft/vcpkg.git
        .\vcpkg\bootstrap-vcpkg.bat -disableMetrics
        .\vcpkg\vcpkg install "opencl:x64-windows-release"
        rm -Recurse -Force .\vcpkg\buildtrees, .\vcpkg\downloads
    }

    # LLVM
    if (-not (Test-Path -Path "{{LLVMPath}}\libclang.dll")) {
        wget "https://github.com/llvm/llvm-project/releases/download/llvmorg-17.0.6/LLVM-17.0.6-win64.exe" -outfile "llvm-win64.exe"
        7z x -y llvm-win64.exe -ollvm
        del "llvm-win64.exe"
    }

    # Adobe SDK
    if (-not (Test-Path -Path ".\AfterEffects")) {
        wget "https://api.gyroflow.xyz/sdk/AdobeSDK.zip" -outfile "AdobeSDK.zip"
        7z x -y AdobeSDK.zip
        del "AdobeSDK.zip"
    }

[macos]
install-deps:
    #!/bin/bash
    set -e

    brew install p7zip pkg-config
    xcode-select --install || true
    rustup target add aarch64-apple-darwin
    rustup target add x86_64-apple-darwin

    mkdir -p {{ExtDir}}
    cd {{ExtDir}}

    # OpenCL
    if [ ! -f "vcpkg/installed/x64-osx-release/lib/OpenCL.lib" ]; then
        git clone --depth 1 https://github.com/Microsoft/vcpkg.git || true
        ./vcpkg/bootstrap-vcpkg.sh -disableMetrics
        ./vcpkg/vcpkg install "opencl:x64-osx-release"
        ./vcpkg/vcpkg install "opencl:arm64-osx"
        rm -rf ./vcpkg/buildtrees ./vcpkg/downloads
    fi

    # Adobe SDK
    if [ ! -f "AfterEffects" ]; then
        curl -L https://api.gyroflow.xyz/sdk/AdobeSDK.zip -o AdobeSDK.zip
        7z x -aoa AdobeSDK.zip
        rm AdobeSDK.zip
    }

[linux]
install-deps:
    #!/bin/bash
    set -e

    sudo apt-get install -y p7zip-full clang libclang-dev pkg-config unzip zip git

    mkdir -p {{ExtDir}}
    cd {{ExtDir}}

    # OpenCL
    if [ ! -f "./lib/libopencv_core4.a" ]; then
        git clone --depth 1 https://github.com/Microsoft/vcpkg.git || true
        ./vcpkg/bootstrap-vcpkg.sh -disableMetrics
        ./vcpkg/vcpkg install "opencl:x64-linux-release"
        rm -rf vcpkg/buildtrees vcpkg/downloads
    fi

    # LLVM
    if [[ ! -d "${LIBCLANG_PATH}" ]]; then
        sudo apt-get install -y libclang-13-dev
    fi

    # Adobe SDK
    if [ ! -f "AfterEffects" ]; then
        curl -L https://api.gyroflow.xyz/sdk/AdobeSDK.zip -o AdobeSDK.zip
        7z x -aoa AdobeSDK.zip
        rm AdobeSDK.zip
    }
