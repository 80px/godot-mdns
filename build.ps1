# build.ps1 — compile godot-mdns natively on Windows and copy outputs into addons/godot-mdns/bin/.
#
# Usage (from the godot-mdns\ directory):
#   .\build.ps1              # Windows x86_64 DLL, debug build  (MSVC toolchain)
#   .\build.ps1 -Release     # Windows x86_64 DLL, release build
#   .\build.ps1 -Gnu         # Windows x86_64 DLL, debug  (GNU/MinGW toolchain — no VS required)
#   .\build.ps1 -Gnu -Release
#   .\build.ps1 -Android     # Android arm64 + arm32 .so (requires cargo-ndk + Android NDK)
#   .\build.ps1 -Android -Release
#
# Note: iOS and macOS cross-compilation are not supported on Windows.
#       Use build.sh on a macOS host for those targets.
#       Linux cross-compilation from Windows is also unsupported here;
#       use WSL and run: ./build.sh --linux
#
# MSVC toolchain prerequisites (default):
#   1. Rust (rustup) — https://rustup.rs
#   2. Visual Studio 2019/2022  OR  Build Tools for Visual Studio:
#      https://aka.ms/vs/17/release/vs_BuildTools.exe
#      During install select: "Desktop development with C++" workload.
#
# GNU toolchain prerequisites (-Gnu, lighter — no Visual Studio needed):
#   1. Rust (rustup) — https://rustup.rs
#   2. MSYS2 — https://www.msys2.org  (installs the MinGW-w64 gcc toolchain)
#      After MSYS2 installs, open the MSYS2 UCRT64 shell and run:
#        pacman -S mingw-w64-ucrt-x86_64-gcc
#      Then add  C:\msys64\ucrt64\bin  to your system PATH.
#   — OR — winlibs standalone GCC: https://winlibs.com  (extract and add to PATH)
#
# Android prerequisites (-Android):
#   1. Rust (rustup) — https://rustup.rs
#   2. Android NDK (standalone or via Android Studio SDK Manager)
#      Set $env:ANDROID_NDK_HOME or $env:ANDROID_NDK_ROOT to the NDK path.
#   3. cargo-ndk:  cargo install cargo-ndk
#   4. Rust targets: rustup target add aarch64-linux-android armv7-linux-androideabi

param(
    [switch]$Release,
    [switch]$Gnu,
    [switch]$Android
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$OutBase   = Join-Path $ScriptDir "addons\godot-mdns\bin"
$Profile   = if ($Release) { "release" } else { "debug" }

if ($Gnu) {
    $Target = "x86_64-pc-windows-gnu"
} else {
    $Target = "x86_64-pc-windows-msvc"
}

# Verify rustup / cargo are available
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "ERROR: cargo not found. Install Rust via rustup: https://rustup.rs" -ForegroundColor Red
    Write-Host "After installation, restart your terminal and re-run this script."
    exit 1
}

# ── Android build ────────────────────────────────────────────────────────────
if ($Android) {
    # cargo-ndk check
    if (-not (Get-Command cargo-ndk -ErrorAction SilentlyContinue)) {
        Write-Host ""
        Write-Host "ERROR: cargo-ndk not found." -ForegroundColor Red
        Write-Host "Install with:  cargo install cargo-ndk"
        Write-Host ""
        exit 1
    }

    # NDK check
    $NdkPath = if ($env:ANDROID_NDK_HOME) { $env:ANDROID_NDK_HOME }
               elseif ($env:ANDROID_NDK_ROOT) { $env:ANDROID_NDK_ROOT }
               else { $null }
    if (-not $NdkPath) {
        Write-Host ""
        Write-Host "ERROR: ANDROID_NDK_HOME or ANDROID_NDK_ROOT is not set." -ForegroundColor Red
        Write-Host "Set it to your Android NDK directory, e.g.:"
        Write-Host "  `$env:ANDROID_NDK_HOME = 'C:\\Android\\ndk\\26.1.10909125'"
        Write-Host ""
        exit 1
    }

    # Ensure targets are installed
    $AndroidTargets = @("aarch64-linux-android", "armv7-linux-androideabi")
    $Installed = rustup target list --installed
    foreach ($t in $AndroidTargets) {
        if ($Installed -notmatch [regex]::Escape($t)) {
            Write-Host "==> Adding Rust target $t ..."
            rustup target add $t
        }
    }

    $Abis = @(
        @{ Abi = "arm64-v8a";   OutArch = "arm64"; Triple = "aarch64-linux-android"   },
        @{ Abi = "armeabi-v7a"; OutArch = "arm32"; Triple = "armv7-linux-androideabi" }
    )

    Push-Location $ScriptDir
    try {
        foreach ($Entry in $Abis) {
            Write-Host ""
            Write-Host "==> [$( (Get-Date).ToString('HH:mm:ss') )] Compiling Android $($Entry.Abi) (profile: $Profile) ..."
            if ($Release) { $buildProfile = "--release" } else { $buildProfile = $null }

            $start = Get-Date
            $ndk_args = @("-t", $Entry.Abi, "--platform", "21")
            if ($Release) { $ndk_args += "--"; $ndk_args += "build"; $ndk_args += "--release" }
            else           { $ndk_args += "--"; $ndk_args += "build" }

            & cargo ndk @ndk_args
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

            $elapsed = [int]((Get-Date) - $start).TotalSeconds
            Write-Host "    Done in ${elapsed}s"

            $Src = Join-Path $ScriptDir "target\$($Entry.Triple)\$Profile\libgodot_mdns.so"
            $Dst = Join-Path $OutBase   "android\$($Entry.OutArch)\$Profile\libgodot_mdns.so"
            $DstDir = Split-Path -Parent $Dst
            if (-not (Test-Path $DstDir)) { New-Item -ItemType Directory -Path $DstDir | Out-Null }

            if (-not (Test-Path $Src)) {
                Write-Error "Build succeeded but .so not found at: $Src"
            }
            Copy-Item -Path $Src -Destination $Dst -Force
            Write-Host "==> Done: $Dst"
        }
    } finally {
        Pop-Location
    }
    exit 0
}

# ── For MSVC target, ensure link.exe is available ──────────────────────────
if (-not $Gnu) {
    if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
        # VS Build Tools installs link.exe but doesn't add it to PATH.
        # Use vswhere.exe (always present after any VS/Build Tools install) to
        # locate the installation and initialize the VS dev environment.
        $vsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
        if (-not (Test-Path $vsWhere)) {
            $vsWhere = Join-Path $env:ProgramFiles "Microsoft Visual Studio\Installer\vswhere.exe"
        }
        if (Test-Path $vsWhere) {
            Write-Host "==> link.exe not in PATH; initializing VS dev environment via vswhere ..."
            $vsPath = & $vsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
            if ($vsPath) {
                $devCmd = Join-Path $vsPath "Common7\Tools\VsDevCmd.bat"
                if (Test-Path $devCmd) {
                    # Import all env vars set by VsDevCmd.bat into this process
                    $envLines = cmd /c "`"$devCmd`" -arch=x64 -no_logo && set" 2>$null
                    foreach ($line in $envLines) {
                        if ($line -match '^([^=]+)=(.*)$') {
                            [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
                        }
                    }
                    # Refresh PATH so Get-Command finds link.exe
                    $env:PATH = [System.Environment]::GetEnvironmentVariable('PATH', 'Process')
                    Write-Host "==> VS dev environment loaded from: $vsPath"
                }
            }
        }

        # Final check after attempted env init
        if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
            Write-Host ""
            Write-Host "ERROR: link.exe (MSVC linker) still not found." -ForegroundColor Red
            Write-Host ""
            Write-Host "Possible fixes:"
            Write-Host "  1. Open a 'Developer PowerShell for VS 2022' and re-run this script."
            Write-Host "  2. Repair your VS Build Tools install and ensure the"
            Write-Host "     'Desktop development with C++' workload is selected."
            Write-Host "  3. Use the GNU toolchain (no VS required):"
            Write-Host "       Install MSYS2 (https://www.msys2.org), then:"
            Write-Host "         pacman -S mingw-w64-ucrt-x86_64-gcc"
            Write-Host "       Add C:\msys64\ucrt64\bin to PATH, then run:"
            Write-Host "         .\build.ps1 -Gnu"
            Write-Host ""
            exit 1
        }
    }
}

# ── For GNU target, check that gcc is on PATH ──────────────────────────────
if ($Gnu) {
    if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) {
        Write-Host ""
        Write-Host "ERROR: gcc not found - required for the GNU target." -ForegroundColor Red
        Write-Host ""
        Write-Host "Install via MSYS2 (recommended):"
        Write-Host "  1. https://www.msys2.org"
        Write-Host "  2. In the MSYS2 UCRT64 shell: pacman -S mingw-w64-ucrt-x86_64-gcc"
        Write-Host "  3. Add C:\msys64\ucrt64\bin to your PATH and restart this terminal."
        Write-Host ""
        Write-Host "  OR download a standalone GCC from https://winlibs.com"
        Write-Host ""
        exit 1
    }
}

# ── Ensure the Rust target is installed ────────────────────────────────────
$installed = rustup target list --installed
if ($installed -notmatch [regex]::Escape($Target)) {
    Write-Host "==> Adding Rust target $Target ..."
    rustup target add $Target
}

# ── Build ───────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "==> [$( (Get-Date).ToString('HH:mm:ss') )] Building godot-mdns (target: $Target, profile: $Profile) ..."
if ($Release) { Write-Host "    (release build — LTO link step can take 3-5 min on first run)" }
$StartTime = Get-Date
Push-Location $ScriptDir
try {
    $cargoArgs = @("build", "--target", $Target)
    if ($Release) { $cargoArgs += "--release" }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
$elapsed = [int]((Get-Date) - $StartTime).TotalSeconds
Write-Host "    Done in ${elapsed}s"

# ── Copy output ─────────────────────────────────────────────────────────────
$Src = Join-Path $ScriptDir "target\$Target\$Profile\godot_mdns.dll"
$Dst = Join-Path $OutBase   "windows\x86_64\$Profile\godot_mdns.dll"

if (-not (Test-Path $Src)) {
    Write-Error "Build succeeded but DLL not found at: $Src"
}

$DstDir = Split-Path -Parent $Dst
if (-not (Test-Path $DstDir)) { New-Item -ItemType Directory -Path $DstDir | Out-Null }

Copy-Item -Path $Src -Destination $Dst -Force
Write-Host "==> Done: $Dst"
