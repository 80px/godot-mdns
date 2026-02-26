# MulticastLock — Godot Android Plugin

A minimal Godot 4 Android plugin that acquires and releases a `WifiManager.MulticastLock`.

## Why this exists

Android drops all multicast UDP packets at the WiFi driver level by default. Since mDNS uses multicast UDP (port 5353), `MdnsBrowser` will never receive any responses on Android unless this lock is held. There is no way to acquire the lock from native (Rust/C++) code — it must be done through the Android Java API.

This plugin exposes three methods to GDScript via `Engine.get_singleton("MulticastLock")`:

| Method | Description |
|---|---|
| `acquire_multicast_lock()` | Acquire the lock. Must be called before `MdnsBrowser.browse()`. Safe to call multiple times. |
| `release_multicast_lock()` | Release the lock. Call when mDNS is no longer needed to conserve battery. |
| `is_multicast_lock_held() -> bool` | Returns whether the lock is currently held. |

The lock is also automatically released in `onMainDestroy` to prevent leaks.

---

## Building locally

### Prerequisites

- JDK 17+
- Android SDK (Gradle needs it via `$ANDROID_HOME` or `local.properties`)
- Godot Android library `.aar` (see below)

### 1 — Download the Godot Android library

The `godot-lib.aar` is not committed. Download it from Godot's releases page and place it in `libs/`:

```bash
mkdir -p libs
curl -L -o libs/godot-lib.aar \
  "https://github.com/godotengine/godot/releases/download/4.3-stable/godot-lib.4.3.stable.template_release.aar"
```

> If you are using a different Godot version, substitute `4.3-stable` for your version in the URL above. The filename format is `godot-lib.<VERSION>.stable.template_release.aar`.

### 2 — Build

```bash
gradle assembleRelease
```

### 3 — Copy into the addon

```bash
cp build/outputs/aar/MulticastLockPlugin-release.aar \
   ../../addons/godot-mdns/android/MulticastLockPlugin.aar
```

---

## Godot export settings

After the `.aar` is in place at `addons/godot-mdns/android/MulticastLockPlugin.aar`:

**Project → Export → Android → Plugins tab:**
- Enable `MulticastLock`

**Project → Export → Android → Permissions tab:**
- Check `CHANGE_WIFI_MULTICAST_STATE`

---

## GDScript usage

```gdscript
func _ready() -> void:
    if OS.get_name() == "Android":
        Engine.get_singleton("MulticastLock").acquire_multicast_lock()

    var browser := MdnsBrowser.new()
    add_child(browser)
    browser.browse("_mygame._tcp.local.")

func _exit_tree() -> void:
    if OS.get_name() == "Android":
        Engine.get_singleton("MulticastLock").release_multicast_lock()
```

---

## CI

The GitHub Actions release workflow (`../.github/workflows/release.yml`) builds this plugin automatically and bundles the `.aar` into the release zip alongside the Rust binaries. You do not need to build it manually when using a published release.
