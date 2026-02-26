# godot-mdns

A [GDExtension](https://docs.godotengine.org/en/stable/tutorials/scripting/gdextension/index.html) written in Rust that adds **mDNS service discovery and advertisement** to Godot 4. Lets clients find LAN-hosted game servers by service type (`_mygame._tcp.local.`) without a DNS server or hard-coded IP addresses.

Uses [`mdns-sd`](https://crates.io/crates/mdns-sd) — a pure-Rust, zero-OS-dependency mDNS implementation. No Avahi, no Bonjour, no system daemon required.

---

## Platform support

| Platform | Supported | Notes |
|---|---|---|
| Linux x86_64 | ✅ | |
| Linux arm64 | ✅ | |
| macOS | ✅ | Universal binary (x86_64 + arm64) |
| Windows x86_64 | ✅ | |
| iOS arm64 | ✅ | Requires Apple multicast entitlement — see [iOS setup](#ios) |
| Android arm64 / arm32 / x86_64 | ✅ | Requires MulticastLock Java plugin — see [Android setup](#android) |
| HTML5 / Web | ❌ | Browser sandbox has no UDP multicast API |

---

## Exposed nodes

| Node | Purpose |
|---|---|
| `MdnsBrowser` | Discovers mDNS services on the LAN; emits signals as services appear/disappear |
| `MdnsAdvertiser` | Registers this machine as a named mDNS service so other nodes can find it |

Both nodes are self-contained: add them as children, connect signals, and free them to stop all mDNS activity automatically.

---

## Quick start (GDScript)

### Discover LAN servers

```gdscript
func _ready() -> void:
    # Acquire the multicast lock on Android BEFORE browsing (no-op on other platforms)
    if OS.get_name() == "Android":
        Engine.get_singleton("MulticastLock").acquire_multicast_lock()

    var browser := MdnsBrowser.new()
    add_child(browser)
    browser.service_discovered.connect(_on_found)
    browser.service_removed.connect(_on_lost)
    browser.browse_error.connect(func(msg): push_error("mDNS browse error: " + msg))
    browser.browse("_mygame._tcp.local.")

func _on_found(name: String, host: String, addresses: PackedStringArray, port: int, txt: Dictionary) -> void:
    print("Server found: %s  at %s:%d" % [name, addresses[0], port])
    print("TXT records: ", txt)

func _on_lost(name: String) -> void:
    print("Server gone: ", name)
```

### Advertise this machine as a server

```gdscript
func _ready() -> void:
    var adv := MdnsAdvertiser.new()
    add_child(adv)
    adv.advertise_error.connect(func(msg): push_error("mDNS advertise error: " + msg))

    var ok := adv.advertise(
        "My Game Server",         # instance name — must be unique on the LAN
        "_mygame._tcp.local.",  # service type — trailing dot is required
        7350,                     # port your server listens on
        { "version": "1.0" }      # optional TXT record key/value pairs
    )
    if not ok:
        push_error("Failed to register mDNS service")
```

---

## API reference

### `MdnsBrowser`

| Member | Kind | Description |
|---|---|---|
| `browse(service_type: String)` | func | Start browsing for `service_type`, e.g. `"_mygame._tcp.local."` The trailing dot is required. Replaces any active browse. |
| `stop_browsing()` | func | Stop the active browse and release the mDNS daemon. Called automatically on `exit_tree`. |
| `is_browsing() -> bool` | func | Returns `true` if a browse is currently active. |
| `service_discovered(name, host, addresses, port, txt)` | signal | Emitted when a service is fully resolved. `addresses` is a `PackedStringArray`, `txt` is a `Dictionary`. |
| `service_removed(name: String)` | signal | Emitted when a previously discovered service disappears. |
| `browse_error(message: String)` | signal | Emitted on internal mDNS errors. |

### `MdnsAdvertiser`

| Member | Kind | Description |
|---|---|---|
| `advertise(instance: String, type: String, port: int, txt: Dictionary) -> bool` | func | Register a service. Returns `false` and emits `advertise_error` on failure. Replaces any active registration. |
| `stop_advertising()` | func | Unregister and release. Called automatically on `exit_tree`. |
| `is_advertising() -> bool` | func | Returns `true` if a service is currently registered. |
| `get_registered_name() -> String` | func | Returns the full mDNS name that was registered, e.g. `"My Game Server._mygame._tcp.local."` |
| `advertise_error(message: String)` | signal | Emitted on internal mDNS errors. |

---

## Building

### Prerequisites

- [Rust](https://rustup.rs/) stable toolchain
- Godot 4.1+
- **iOS only:** macOS host with Xcode installed
- **Android only:** Android NDK + [`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk)

### Local build — macOS / Linux (host platform)

```bash
cd godot-mdns
./build.sh            # debug build
./build.sh --release  # release build
```

Binaries are placed directly into `addons/godot-mdns/bin/` where Godot resolves them via `res://`.

### Local build — Windows (native)

Use the PowerShell script. Run from the `godot-mdns\` directory:

```powershell
# MSVC toolchain (Visual Studio / Build Tools required)
.\build.ps1
.\build.ps1 -Release

# GNU toolchain (MSYS2 / MinGW, no Visual Studio required)
.\build.ps1 -Gnu
.\build.ps1 -Gnu -Release
```

**MSVC prerequisites:** [Visual Studio Build Tools](https://aka.ms/vs/17/release/vs_BuildTools.exe) with the "Desktop development with C++" workload.  
**GNU prerequisites:** [MSYS2](https://www.msys2.org/) — run `pacman -S mingw-w64-ucrt-x86_64-gcc` in the UCRT64 shell and add `C:\msys64\ucrt64\bin` to `PATH`.

### Local build — Windows (cross-compile from macOS/Linux/WSL)

```bash
# macOS
brew install mingw-w64
# Linux / WSL
sudo apt install gcc-mingw-w64-x86-64

rustup target add x86_64-pc-windows-gnu
./build.sh --windows
./build.sh --windows --release
```

> **WSL note:** `./build.sh` with no flags auto-detects WSL and defaults to `--windows`, since that's the artefact needed on the host machine. Pass `--linux` explicitly to build a Linux `.so` from WSL instead.

### Local build — iOS (macOS host required)

```bash
rustup target add aarch64-apple-ios
./build.sh --ios
./build.sh --ios --release
```

Output: `addons/godot-mdns/bin/ios/arm64/<profile>/libgodot_mdns.a`

### Local build — Android

**macOS / Linux / WSL:**
```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
export ANDROID_NDK_HOME=/path/to/your/ndk

./build.sh --android
./build.sh --android --release
```

**Windows (PowerShell):**
```powershell
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
$env:ANDROID_NDK_HOME = "C:\path\to\your\ndk"

.\build.ps1 -Android
.\build.ps1 -Android -Release
```

Output: `addons/godot-mdns/bin/android/arm64/<profile>/libgodot_mdns.so`, `arm32/`, and `x86_64/`

Also build the MulticastLock Java plugin (one-time, required for Android):
```bash
cd android-plugin
# Download godot-lib.aar first — see android-plugin/README.md
gradle assembleRelease
cp build/outputs/aar/MulticastLockPlugin-release.aar ../addons/godot-mdns/android/MulticastLockPlugin.aar
```

### CI

Two GitHub Actions workflows run automatically:

| Workflow | Trigger | What it does |
|---|---|---|
| **Build** | Push to `main` (when `src/`, `Cargo.toml`, or `Cargo.lock` change), any PR, or manual dispatch | Debug-only compile check across all 7 platforms. Confirms nothing is broken cross-platform. No artifacts produced. |
| **Release** | Pushing a `v*` tag | Release-profile build for all platforms, packages a zip, and publishes a GitHub Release. |

Day-to-day pushes to `main` run a fast compile check. The release workflow only fires when you deliberately tag a version:

```bash
git tag v0.2.0
git push origin v0.2.0
```

GitHub Actions builds all 7 targets in parallel and publishes a `godot-mdns-v0.2.0.zip` as a GitHub Release. The zip extracts directly into your Godot project root.

---

## Releasing

The version string lives in **two places** — keep them in sync before tagging:

| File | Line |
|---|---|
| [`Cargo.toml`](Cargo.toml) | `version = "x.y.z"` under `[package]` |
| [`addons/godot-mdns/plugin.cfg`](addons/godot-mdns/plugin.cfg) | `version="x.y.z"` |

Steps to cut a release:

```bash
# 1. Update both files to the new version
# 2. Commit
git add Cargo.toml addons/godot-mdns/plugin.cfg
git commit -m "chore: bump version to 0.2.0"

# 3. Tag and push — this triggers the GitHub Actions release workflow
git push                  # push the version bump commit first
git tag v0.2.0
git push origin v0.2.0    # push the tag explicitly — this triggers the workflow
```

---

## Integrating into a Godot project

1. Build or download binaries for your target platform(s).
2. `addons/godot-mdns/godot-mdns.gdextension` and `addons/godot-mdns/plugin.cfg` are already committed in this repo — no extra copy needed.
3. Restart Godot. `MdnsBrowser` and `MdnsAdvertiser` appear in the **Add Node** dialog automatically.

---

## Platform-specific setup

### Android

#### Why a Java plugin is needed

Android silently drops all inbound multicast UDP packets at the WiFi driver level by default. Because mDNS uses multicast UDP on port 5353, `MdnsBrowser` will never receive any responses on Android without intervention — the datagrams are discarded before the Rust code ever sees them.

The fix is a `WifiManager.MulticastLock`. While the lock is held, Android instructs the WiFi hardware to pass multicast packets up the network stack to the application. The critical constraint is that this lock **can only be acquired through the Android Java API** — there is no NDK (C/C++ or Rust) equivalent. Native code alone cannot enable multicast reception.

#### What the bundled plugin does

The `android-plugin/` directory contains a minimal Godot 4 Android plugin written in Java (`MulticastLockPlugin.java`). It extends `GodotPlugin`, which lets Godot load it as a singleton at runtime. The plugin:

1. Registers itself under the name `"MulticastLock"` (via `getPluginName()`).
2. Exposes three methods annotated with `@UsedByGodot` so GDScript can call them:
   - `acquire_multicast_lock()` — obtains a `WifiManager.MulticastLock` tagged `"godot-mdns"` with reference counting enabled and calls `acquire()` on it.
   - `release_multicast_lock()` — calls `release()` and nulls the reference.
   - `is_multicast_lock_held() -> bool` — returns the current lock state.
3. Overrides `onMainDestroy()` to always release the lock when the app is destroyed, preventing a system-level lock leak.

The plugin is packaged as an Android Archive (`.aar`) and declared to Godot via `addons/godot-mdns/android/MulticastLockPlugin.gdap`, which maps the plugin name `"MulticastLock"` to `MulticastLockPlugin.aar`.

#### Build pipeline summary

```
build.sh --android  (or build.ps1 -Android)
  └─ cargo ndk -t arm64-v8a   → target/aarch64-linux-android/<profile>/libgodot_mdns.so
  └─ cargo ndk -t armeabi-v7a → target/armv7-linux-androideabi/<profile>/libgodot_mdns.so
  └─ cargo ndk -t x86_64      → target/x86_64-linux-android/<profile>/libgodot_mdns.so

android-plugin/ (one-time)
  └─ gradle assembleRelease   → MulticastLockPlugin-release.aar
                              → copied to addons/godot-mdns/android/MulticastLockPlugin.aar

Godot Android export
  ├─ loads libgodot_mdns.so  (GDExtension — provides MdnsBrowser / MdnsAdvertiser nodes)
  ├─ loads MulticastLockPlugin.aar  (GodotPlugin — provides the MulticastLock singleton)
  └─ merges CHANGE_WIFI_MULTICAST_STATE permission into the final APK/AAB manifest
```

`cargo ndk` wraps the Android NDK cross-compiler so that ordinary `cargo build` invocations target the correct Android ABI and NDK sysroot. The resulting `.so` files are standard GDExtension shared libraries — Godot loads them automatically based on the `[libraries]` entries in `godot-mdns.gdextension`.

#### Export settings

**In the Godot Export dialog → Android:**

1. **Plugins tab** — enable `MulticastLock`
2. **Permissions tab** — check `CHANGE_WIFI_MULTICAST_STATE`
3. Ensure `addons/godot-mdns/android/MulticastLockPlugin.aar` is present (built from `android-plugin/` or downloaded from a release)

#### GDScript usage

Acquire the lock **before** calling `MdnsBrowser.browse()`:

```gdscript
if OS.get_name() == "Android":
    Engine.get_singleton("MulticastLock").acquire_multicast_lock()
```

Release it when mDNS is no longer needed to conserve battery:

```gdscript
if OS.get_name() == "Android":
    Engine.get_singleton("MulticastLock").release_multicast_lock()
```

The lock is also automatically released in `onMainDestroy()` as a safety net, but explicit release is preferred.

### iOS

iOS requires two export settings and an Apple entitlement approval before you can ship.

**In the Godot Export dialog → iOS → Additional Plist Content**, add:

```xml
<key>NSLocalNetworkUsageDescription</key>
<string>Used to discover local game servers on your network.</string>
```

**In the Godot Export dialog → iOS → Custom Entitlements**, add:

```xml
<key>com.apple.developer.networking.multicast</key>
<true/>
```

> **Apple approval required:** The `com.apple.developer.networking.multicast` entitlement must be explicitly approved by Apple before your app can send or receive IP multicast on iOS. Request it at the [Multicast Networking Entitlement Request](https://developer.apple.com/contact/request/networking-multicast) page. Without approval your binary will be rejected at App Store submission.

---

## Why not HTML5?

The browser sandbox provides no raw UDP socket API, so multicast DNS is architecturally impossible in web exports. For web-based server discovery, use an HTTP relay endpoint that servers register with on startup.

---

## License

MIT or Apache-2.0 (same dual licence as [`mdns-sd`](https://crates.io/crates/mdns-sd))
