//! godot-mdns — GDExtension exposing mDNS service discovery and advertisement to Godot 4.
//!
//! Exposes two nodes:
//!   - [`MdnsBrowser`]   — discover services on the LAN (emits signals each frame via polling)
//!   - [`MdnsAdvertiser`] — announce a service so other nodes/devices can find this machine
//!
//! Both nodes are self-contained: add them as children, connect signals, call the exposed
//! functions, and remove/free them to stop mDNS activity automatically.
//!
//! ## IMPORTANT: shared daemon design
//!
//! `ServiceDaemon::new()` binds a UDP socket on port 5353 and starts a background thread.
//! Creating two daemons in the same process means two sockets compete for the same multicast
//! port.  On macOS SO_REUSEPORT distributes packets between them non-deterministically; on
//! Windows the second bind often silently fails.  The result is that discovery announcements
//! from a remote host land on whichever local daemon happens to receive them, and the *other*
//! daemon (browse or advertise) never sees them — producing intermittent or one-way discovery.
//!
//! The fix: use a single process-global `ServiceDaemon` (stored in `SHARED_DAEMON`) that both
//! `MdnsBrowser` and `MdnsAdvertiser` clone handles from.  `ServiceDaemon` is internally
//! `Arc`-backed so `.clone()` is cheap and all clones share the same background thread and
//! socket.  Only the Android `iface_ip` path creates a dedicated second daemon because that
//! path calls `disable_interface(All)` + `enable_interface(specific)` which would break any
//! co-running advertiser — and Android devices never run `MdnsAdvertiser`.

use godot::prelude::*;
use mdns_sd::{IfKind, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Shared daemon
// ---------------------------------------------------------------------------

/// Process-global mDNS daemon shared by both `MdnsBrowser` and `MdnsAdvertiser`.
/// Lazily initialised on first call to `shared_daemon()`.
static SHARED_DAEMON: OnceLock<Mutex<Option<ServiceDaemon>>> = OnceLock::new();

/// Returns a clone of the shared `ServiceDaemon`, creating it on first call.
///
/// Returns `Err` with a description string if the daemon could not be created.
fn shared_daemon() -> Result<ServiceDaemon, String> {
    let mutex = SHARED_DAEMON.get_or_init(|| Mutex::new(None));
    let mut guard = mutex.lock().map_err(|e| format!("shared daemon mutex poisoned: {e}"))?;
    if guard.is_none() {
        *guard = Some(
            ServiceDaemon::new()
                .map_err(|e| format!("Failed to create shared mDNS daemon: {e}"))?,
        );
    }
    Ok(guard.as_ref().unwrap().clone())
}

// ---------------------------------------------------------------------------
// Extension entry-point
// ---------------------------------------------------------------------------

struct GodotMdnsExtension;

#[gdextension]
unsafe impl ExtensionLibrary for GodotMdnsExtension {}

// ---------------------------------------------------------------------------
// MdnsBrowser
// ---------------------------------------------------------------------------

/// Browses the LAN for an mDNS service type and emits signals when services
/// are discovered or removed.
///
/// ## GDScript example
/// ```gdscript
/// var browser := MdnsBrowser.new()
/// add_child(browser)
/// browser.service_discovered.connect(_on_service_discovered)
/// browser.service_removed.connect(_on_service_removed)
/// browser.browse("_mygame._tcp.local.")
///
/// func _on_service_discovered(name, host, addresses, port, txt):
///     print("Found server: ", name, " at ", addresses, ":", port)
///
/// func _on_service_removed(name):
///     print("Server gone: ", name)
/// ```
#[derive(GodotClass)]
#[class(base = Node)]
pub struct MdnsBrowser {
    /// Clone of the shared daemon (or a private daemon when `iface_ip` is set).
    /// Holding a clone keeps the reference alive; dropping it without calling
    /// `shutdown()` is safe — the daemon only stops when every clone is dropped.
    daemon: Option<ServiceDaemon>,
    receiver: Option<mdns_sd::Receiver<ServiceEvent>>,
    /// The service type currently being browsed (e.g. `"_mygame._tcp.local."`).
    /// Stored so `stop_browsing()` can call `daemon.stop_browse()` to clean up
    /// the browse subscription in the shared daemon.
    service_type: Option<String>,
    /// Optional IP address string to restrict the daemon to a single network
    /// interface.  Set this before calling `browse()`.  On Android the WiFi
    /// interface IP must be supplied explicitly because the driver will not
    /// deliver multicast packets to sockets joined on the wrong interface even
    /// after a MulticastLock is acquired.
    ///
    /// When set, a *private* daemon is created for this browser instead of
    /// the shared one, because `disable_interface(All)` would affect any
    /// co-running `MdnsAdvertiser`.  Android devices never run
    /// `MdnsAdvertiser` so this is safe in practice.
    iface_ip: Option<String>,
    base: Base<Node>,
}

#[godot_api]
impl INode for MdnsBrowser {
    fn init(base: Base<Node>) -> Self {
        Self {
            daemon: None,
            receiver: None,
            service_type: None,
            iface_ip: None,
            base,
        }
    }

    /// Poll the mDNS channel every frame — non-blocking, drains all pending events.
    fn process(&mut self, _delta: f64) {
        self.drain_events();
    }

    /// Automatically stop browsing when the node is removed from the scene tree.
    fn exit_tree(&mut self) {
        self.stop_browsing();
    }
}

#[godot_api]
impl MdnsBrowser {
    // ── Signals ──────────────────────────────────────────────────────────────

    /// Emitted when a service has been fully resolved (IP addresses are known).
    ///
    /// Parameters:
    ///   name      — full service name, e.g. "My Server._mygame._tcp.local."
    ///   host      — hostname, e.g. "marks-pc.local."
    ///   addresses — array of IP address strings (IPv4 and/or IPv6)
    ///   port      — TCP/UDP port as int
    ///   txt       — VarDictionary of TXT record key→value strings
    #[signal]
    fn service_discovered(
        name: GString,
        host: GString,
        addresses: PackedStringArray,
        port: i64,
        txt: VarDictionary,
    );

    /// Emitted when a previously discovered service disappears from the LAN.
    ///
    /// Parameters:
    ///   name — full service name that was removed
    #[signal]
    fn service_removed(name: GString);

    /// Emitted if an internal mDNS error occurs.
    #[signal]
    fn browse_error(message: GString);

    // ── Methods ──────────────────────────────────────────────────────────────

    /// Pin the daemon to a single network interface by its IP address string
    /// (e.g. `"192.168.1.42"`).  Call this **before** `browse()`.  Passing an
    /// empty string clears any previously set hint and reverts to all-interface
    /// auto-detection.
    ///
    /// On Android this is required because `mdns-sd`'s default all-interface
    /// socket binding does not reliably receive multicast traffic through the
    /// WiFi driver even when a MulticastLock is held.  Restricting to the
    /// correct WiFi IP ensures the daemon's socket joins the 224.0.0.251
    /// multicast group on exactly that interface.
    ///
    /// When an interface IP is set, this browser creates its own private daemon
    /// rather than using the shared one.
    #[func]
    fn set_interface(&mut self, iface_ip: GString) {
        let s = iface_ip.to_string();
        self.iface_ip = if s.is_empty() { None } else { Some(s) };
    }

    /// Start browsing for `service_type`, e.g. `"_mygame._tcp.local."`.
    ///
    /// Calling `browse()` again while already browsing stops the previous search first.
    /// The trailing dot in the service type is required by the mDNS spec.
    #[func]
    fn browse(&mut self, service_type: GString) {
        // Clean up any existing browse session.
        self.stop_browsing();

        // Obtain a daemon handle.  If an interface IP is pinned (Android path),
        // create a private daemon so we can restrict its interface without
        // affecting the shared daemon that MdnsAdvertiser may be using.
        // For all other platforms, clone the shared daemon to avoid dual-socket conflicts.
        let daemon = if let Some(ref ip_str) = self.iface_ip.clone() {
            match ip_str.parse::<IpAddr>() {
                Ok(ip) => {
                    match ServiceDaemon::new() {
                        Ok(d) => {
                            if let Err(e) = d.disable_interface(IfKind::All) {
                                self.emit_browse_error(format!("disable_interface(All) failed: {e}"));
                            }
                            if let Err(e) = d.enable_interface(IfKind::Addr(ip)) {
                                self.emit_browse_error(format!("enable_interface({ip}) failed: {e}"));
                            }
                            d
                        }
                        Err(e) => {
                            self.emit_browse_error(format!("Failed to create mDNS daemon: {e}"));
                            return;
                        }
                    }
                }
                Err(_) => {
                    self.emit_browse_error(format!("set_interface: invalid IP '{}'", ip_str));
                    return;
                }
            }
        } else {
            match shared_daemon() {
                Ok(d) => d,
                Err(e) => {
                    self.emit_browse_error(e);
                    return;
                }
            }
        };

        let receiver = match daemon.browse(service_type.to_string().as_str()) {
            Ok(r) => r,
            Err(e) => {
                self.emit_browse_error(format!("Failed to start mDNS browse: {e}"));
                // Drop private daemon if it was created (shared one lives on).
                return;
            }
        };

        self.service_type = Some(service_type.to_string());
        self.daemon = Some(daemon);
        self.receiver = Some(receiver);
    }

    /// Stop the active browse and release this node's daemon handle.
    ///
    /// For the shared daemon, dropping the clone does not shut down the background
    /// thread — other users (e.g. `MdnsAdvertiser`) keep their own clones alive.
    /// For the private Android daemon, dropping it here shuts it down because this
    /// was the only clone.
    #[func]
    fn stop_browsing(&mut self) {
        // Tell the daemon to stop the browse subscription so it no longer sends
        // multicast queries or queues events for this service type.
        if let (Some(daemon), Some(svc_type)) = (&self.daemon, &self.service_type) {
            let _ = daemon.stop_browse(svc_type);
        }
        // Drop receiver first so the browse channel flushes cleanly.
        self.receiver = None;
        self.service_type = None;
        // Drop daemon clone — does not shutdown shared daemon; only shuts down
        // the private Android daemon (which has no other live clones).
        self.daemon = None;
    }

    /// Returns `true` if a browse is currently active.
    #[func]
    fn is_browsing(&self) -> bool {
        self.receiver.is_some()
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Non-blocking drain — processes all queued events without blocking the main thread.
    fn drain_events(&mut self) {
        loop {
            let event = match &self.receiver {
                Some(rx) => match rx.try_recv() {
                    Ok(ev) => ev,
                    Err(_) => break, // Empty or disconnected — nothing more to process.
                },
                None => break,
            };
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                self.on_service_resolved(info);
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                self.base_mut().emit_signal(
                    "service_removed",
                    &[GString::from(&fullname).to_variant()],
                );
            }
            // SearchStarted / SearchStopped / ServiceFound are informational; ignored here.
            _ => {}
        }
    }

    fn on_service_resolved(&mut self, info: Box<ResolvedService>) {
        let name = GString::from(info.get_fullname());
        let host = GString::from(info.get_hostname());
        let port = info.get_port() as i64;

        // Collect into a Vec and sort so IPv4 addresses always come before IPv6.
        // `get_addresses()` iterates a HashSet whose order is non-deterministic;
        // without this sort `addresses[0]` can be an IPv6 link-local address
        // (fe80::…) that Godot/Nakama cannot use as a plain host string.
        // mdns-sd 0.18+ returns ScopedIp; convert to plain IpAddr for Godot strings.
        let mut sorted_addrs: Vec<IpAddr> = info.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
        sorted_addrs.sort_by_key(|a| if a.is_ipv4() { 0u8 } else { 1u8 });

        let mut addresses = PackedStringArray::new();
        for addr in &sorted_addrs {
            addresses.push(addr.to_string().as_str());
        }

        let mut txt = VarDictionary::new();
        for prop in info.get_properties().iter() {
            txt.set(
                GString::from(prop.key()),
                GString::from(prop.val_str()),
            );
        }

        self.base_mut().emit_signal(
            "service_discovered",
            &[
                name.to_variant(),
                host.to_variant(),
                addresses.to_variant(),
                port.to_variant(),
                txt.to_variant(),
            ],
        );
    }

    fn emit_browse_error(&mut self, msg: String) {
        self.base_mut()
            .emit_signal("browse_error", &[GString::from(msg.as_str()).to_variant()]);
    }
}

// ---------------------------------------------------------------------------
// MdnsAdvertiser
// ---------------------------------------------------------------------------

/// Advertises an mDNS service so that other nodes/devices on the LAN can
/// discover this machine via [`MdnsBrowser`].
///
/// ## GDScript example
/// ```gdscript
/// var adv := MdnsAdvertiser.new()
/// add_child(adv)
/// adv.advertise_error.connect(func(msg): push_error("mDNS: " + msg))
///
/// # Announce the Nakama server port so clients on the LAN can find it
/// var ok := adv.advertise("My Game Server", "_mygame._tcp.local.", 7350, {
///     "version": "1.0",
///     "region": "eu-west",
/// })
/// if ok:
///     print("mDNS service registered")
/// ```
#[derive(GodotClass)]
#[class(base = Node)]
pub struct MdnsAdvertiser {
    /// Clone of the shared daemon.  Kept alive so the service stays registered.
    /// Dropped (without `shutdown()`) in `stop_advertising()`.
    daemon: Option<ServiceDaemon>,
    fullname: Option<String>,
    base: Base<Node>,
}

#[godot_api]
impl INode for MdnsAdvertiser {
    fn init(base: Base<Node>) -> Self {
        Self {
            daemon: None,
            fullname: None,
            base,
        }
    }

    /// Automatically unregister and clean up when the node leaves the tree.
    fn exit_tree(&mut self) {
        self.stop_advertising();
    }
}

#[godot_api]
impl MdnsAdvertiser {
    // ── Signals ──────────────────────────────────────────────────────────────

    /// Emitted if registration or any internal mDNS error occurs.
    #[signal]
    fn advertise_error(message: GString);

    // ── Methods ──────────────────────────────────────────────────────────────

    /// Register an mDNS service.
    ///
    /// - `instance_name` — human-readable label, e.g. `"Mark's Server"`.  
    ///   Must be unique among instances of the same `service_type` on the LAN.
    /// - `service_type`  — e.g. `"_mygame._tcp.local."` (trailing dot required).
    /// - `port`          — the port your service actually listens on.
    /// - `txt_records`   — optional String→String Dictionary added to the TXT record.
    ///
    /// Returns `true` on success. On failure, `false` is returned and
    /// `advertise_error` is emitted with a description.
    ///
    /// Calling `advertise()` while already advertising quietly stops the
    /// previous registration first.
    #[func]
    fn advertise(
        &mut self,
        instance_name: GString,
        service_type: GString,
        port: i64,
        txt_records: VarDictionary,
    ) -> bool {
        self.stop_advertising();

        let daemon = match shared_daemon() {
            Ok(d) => d,
            Err(e) => {
                self.emit_adv_error(e);
                return false;
            }
        };

        // Build TXT record properties.
        // We need owned Strings before we can hand out &str slices.
        let owned_props: Vec<(String, String)> = txt_records
            .iter_shared()
            .filter_map(|(k, v)| {
                let key = k.try_to::<GString>().ok()?.to_string();
                let val = v.try_to::<GString>().ok()?.to_string();
                Some((key, val))
            })
            .collect();

        let props: Vec<(&str, &str)> = owned_props
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let port_u16 = port.clamp(1, 65535) as u16;

        // Build a "hostname.local." string for this machine.
        let hostname_local = format!("{}.local.", get_hostname());

        let info = match ServiceInfo::new(
            service_type.to_string().as_str(),
            instance_name.to_string().as_str(),
            hostname_local.as_str(),
            // Empty string → mdns-sd resolves all local interface IPs automatically.
            "",
            port_u16,
            props.as_slice(),
        ) {
            Ok(i) => i,
            Err(e) => {
                self.emit_adv_error(format!("Failed to build ServiceInfo: {e}"));
                return false;
            }
        };

        let fullname = info.get_fullname().to_string();

        if let Err(e) = daemon.register(info) {
            self.emit_adv_error(format!("Failed to register mDNS service: {e}"));
            return false;
        }

        self.fullname = Some(fullname);
        self.daemon = Some(daemon);
        true
    }

    /// Unregister the advertised service and release this node's daemon handle.
    ///
    /// The shared daemon itself stays alive as long as any other clone exists
    /// (e.g. a running `MdnsBrowser`).  Dropping the clone here does not shut
    /// down the background thread.
    ///
    /// Called automatically from `exit_tree`; safe to call manually at any time.
    #[func]
    fn stop_advertising(&mut self) {
        if let (Some(daemon), Some(name)) = (&self.daemon, &self.fullname) {
            let _ = daemon.unregister(name);
        }
        self.fullname = None;
        // Drop clone — does not shutdown shared daemon.
        self.daemon = None;
    }

    /// Returns `true` if the service is currently being advertised.
    #[func]
    fn is_advertising(&self) -> bool {
        self.daemon.is_some()
    }

    /// Returns the full mDNS service name that was registered, or an empty string.
    #[func]
    fn get_registered_name(&self) -> GString {
        GString::from(self.fullname.as_deref().unwrap_or(""))
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn emit_adv_error(&mut self, msg: String) {
        self.base_mut()
            .emit_signal("advertise_error", &[GString::from(msg.as_str()).to_variant()]);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the local machine hostname without a domain suffix.
fn get_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string())
}
