package com.godotmdns;

import android.content.Context;
import android.net.wifi.WifiManager;
import android.util.Log;

import androidx.annotation.NonNull;

import org.godotengine.godot.Godot;
import org.godotengine.godot.plugin.GodotPlugin;
import org.godotengine.godot.plugin.UsedByGodot;

/**
 * Godot 4 Android plugin that acquires and releases a WifiManager.MulticastLock.
 *
 * Android silently drops multicast UDP packets (used by mDNS) at the WiFi driver level
 * unless this lock is held. This plugin must be enabled in the Godot Android export
 * settings and the lock must be acquired before starting MdnsBrowser.
 *
 * GDScript usage:
 *   if OS.get_name() == "Android":
 *       Engine.get_singleton("MulticastLock").acquire_multicast_lock()
 *
 * Required manifest permission (set in Godot Export → Android → Permissions):
 *   android.permission.CHANGE_WIFI_MULTICAST_STATE
 */
public class MulticastLockPlugin extends GodotPlugin {

    private static final String TAG = "MulticastLockPlugin";
    private WifiManager.MulticastLock multicastLock;

    public MulticastLockPlugin(Godot godot) {
        super(godot);
    }

    @NonNull
    @Override
    public String getPluginName() {
        return "MulticastLock";
    }

    /**
     * Acquire the multicast lock, allowing the OS to deliver multicast UDP packets
     * to this process. Must be called before MdnsBrowser.browse().
     *
     * Safe to call multiple times — subsequent calls are no-ops if the lock is already held.
     */
    @UsedByGodot
    public void acquire_multicast_lock() {
        if (multicastLock != null && multicastLock.isHeld()) {
            Log.d(TAG, "MulticastLock already held, skipping acquire.");
            return;
        }
        try {
            WifiManager wifi = (WifiManager) getActivity()
                    .getApplicationContext()
                    .getSystemService(Context.WIFI_SERVICE);
            if (wifi == null) {
                Log.e(TAG, "Could not obtain WifiManager — multicast may not work.");
                return;
            }
            multicastLock = wifi.createMulticastLock("godot-mdns");
            multicastLock.setReferenceCounted(true);
            multicastLock.acquire();
            Log.d(TAG, "MulticastLock acquired.");
        } catch (Exception e) {
            Log.e(TAG, "Failed to acquire MulticastLock: " + e.getMessage());
        }
    }

    /**
     * Release the multicast lock. Call this when mDNS activity is no longer needed
     * (e.g. when leaving the LAN server browser screen) to conserve battery.
     */
    @UsedByGodot
    public void release_multicast_lock() {
        if (multicastLock != null && multicastLock.isHeld()) {
            multicastLock.release();
            Log.d(TAG, "MulticastLock released.");
        }
        multicastLock = null;
    }

    /**
     * Returns true if the MulticastLock is currently held.
     */
    @UsedByGodot
    public boolean is_multicast_lock_held() {
        return multicastLock != null && multicastLock.isHeld();
    }

    @Override
    public void onMainDestroy() {
        // Always release on app destruction to avoid leaking the lock.
        release_multicast_lock();
        super.onMainDestroy();
    }
}
