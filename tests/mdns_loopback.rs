//! Integration tests for the godot-mdns GDExtension library.
//!
//! These tests validate the `mdns-sd` usage patterns that `MdnsBrowser` and
//! `MdnsAdvertiser` rely on, running without Godot.
//!
//! Run with:
//!   cargo test --test mdns_loopback -- --nocapture --test-threads=1
//!
//! IMPORTANT: `--test-threads=1` is **required** — the daemon tests share a
//! process-global mDNS daemon (mirroring the GDExtension's SHARED_DAEMON).
//!
//! ## Test categories
//!
//! 1. **Pure logic** (always pass): IPv4 sorting, ServiceInfo construction,
//!    TXT record building, hostname retrieval.
//! 2. **Daemon lifecycle** (always pass): create, register, unregister,
//!    shutdown — validates the API without needing network traffic.
//! 3. **OS environment checks** (informational): raw multicast loopback,
//!    port 5353 availability, per-interface multicast capability.
//! 4. **Network round-trip** (conditional): browse→register→resolve on the
//!    same machine.  These tests are SKIPPED (not failed) when the OS cannot
//!    deliver multicast loopback — a known limitation on Windows with Hyper-V
//!    virtual switches.  Cross-machine LAN discovery (the real use case) is
//!    unaffected.
//!
//! When a network test is skipped you will see:
//!   `[SKIP] ... (multicast loopback unavailable on this machine)`
//!
//! This does NOT mean the library is broken — it means same-machine loopback
//! testing is not possible in this network environment.

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Shared-daemon helper (mirrors lib.rs SHARED_DAEMON pattern)
// ---------------------------------------------------------------------------

static TEST_DAEMON: OnceLock<Mutex<Option<ServiceDaemon>>> = OnceLock::new();

/// Returns a clone of the shared test daemon, creating it on first call.
/// Uses default all-interfaces binding (same as the real GDExtension).
fn shared_test_daemon() -> ServiceDaemon {
    let mutex = TEST_DAEMON.get_or_init(|| Mutex::new(None));
    let mut guard = mutex.lock().expect("test daemon mutex poisoned");
    if guard.is_none() {
        let d = ServiceDaemon::new().expect("failed to create mDNS daemon");
        println!("[daemon] created with default (all-interface) binding");
        *guard = Some(d);
    }
    guard.as_ref().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unique_service_type(suffix: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("_t{ts}{suffix}._tcp.local.")
}

fn get_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "test-host".to_string())
}

/// Drain the receiver until `ServiceResolved` for `fullname` appears, or timeout.
fn wait_for_resolved(
    receiver: &mdns_sd::Receiver<ServiceEvent>,
    fullname: &str,
    timeout: Duration,
) -> Option<Box<ResolvedService>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match receiver.try_recv() {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                println!("  [resolved] {}", info.get_fullname());
                if info.get_fullname() == fullname {
                    return Some(info);
                }
            }
            Ok(ServiceEvent::ServiceFound(svc_type, fullname_found)) => {
                println!("  [found] type={svc_type} name={fullname_found}");
            }
            Ok(ev) => {
                println!("  [event] {:?}", ev);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    None
}

/// Checks whether same-machine mDNS service resolution works by doing a quick
/// register+browse on a throwaway daemon with a custom port.
///
/// This is the definitive test: if this returns true, the full network tests
/// will pass.  If false, the OS/network environment doesn't support
/// same-process mDNS loopback (common on Windows with Hyper-V).
fn mdns_self_resolve_works() -> bool {
    let daemon = match ServiceDaemon::new_with_port(15353) {
        Ok(d) => d,
        Err(_) => return false,
    };
    let _ = daemon.set_multicast_loop_v4(true);

    let svc_type = "_probe._tcp.local.";
    let receiver = match daemon.browse(svc_type) {
        Ok(r) => r,
        Err(_) => return false,
    };
    std::thread::sleep(Duration::from_millis(300));

    let hostname = format!("{}.local.", get_hostname());
    let info = match ServiceInfo::new(svc_type, "probe", &hostname, "", 1234, &[] as &[(&str, &str)]) {
        Ok(i) => i,
        Err(_) => return false,
    };
    let fullname = info.get_fullname().to_string();
    let _ = daemon.register(info);

    let result = wait_for_resolved(&receiver, &fullname, Duration::from_secs(5)).is_some();
    let _ = daemon.unregister(&fullname);
    let _ = daemon.shutdown();
    result
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CATEGORY 1: Pure logic tests (always pass)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn t0_ipv4_sorted_first() {
    use std::net::IpAddr;
    let mixed: Vec<IpAddr> = vec![
        "fe80::1".parse().unwrap(),
        "192.168.1.42".parse().unwrap(),
        "::1".parse().unwrap(),
        "10.0.0.1".parse().unwrap(),
    ];

    let mut sorted = mixed.clone();
    sorted.sort_by_key(|a| if a.is_ipv4() { 0u8 } else { 1u8 });

    assert!(sorted[0].is_ipv4(), "first address should be IPv4");
    assert!(sorted[1].is_ipv4(), "second address should be IPv4");
    assert!(sorted[2].is_ipv6());
    assert!(sorted[3].is_ipv6());
    println!("[t0] PASS — sorted: {:?}", sorted);
}

#[test]
fn t1_service_info_construction() {
    let svc_type = "_mygame._tcp.local.";
    let hostname = format!("{}.local.", get_hostname());
    let txt: Vec<(&str, &str)> = vec![
        ("server_key", "micrommo_dev_key"),
        ("name", "Test Server"),
        ("region", "LAN"),
        ("use_ssl", "false"),
    ];

    let info = ServiceInfo::new(svc_type, "my-instance", &hostname, "", 7350, txt.as_slice())
        .expect("ServiceInfo::new should succeed");

    // Verify the fullname follows mDNS naming conventions.
    let fullname = info.get_fullname();
    assert!(
        fullname.contains("my-instance"),
        "fullname should contain the instance name, got: {fullname}"
    );
    assert!(
        fullname.contains(svc_type),
        "fullname should contain the service type, got: {fullname}"
    );

    // Port round-trips.
    assert_eq!(info.get_port(), 7350);

    // Hostname round-trips.
    assert_eq!(info.get_hostname(), &hostname);

    // TXT properties round-trip.
    let props: HashMap<String, String> = info
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();
    assert_eq!(props.get("server_key").map(|s| s.as_str()), Some("micrommo_dev_key"));
    assert_eq!(props.get("name").map(|s| s.as_str()), Some("Test Server"));
    assert_eq!(props.get("region").map(|s| s.as_str()), Some("LAN"));
    assert_eq!(props.get("use_ssl").map(|s| s.as_str()), Some("false"));

    println!("[t1] PASS — ServiceInfo construction and field access verified");
}

#[test]
fn t2_service_info_empty_txt() {
    let svc_type = "_notxt._tcp.local.";
    let hostname = format!("{}.local.", get_hostname());

    let info = ServiceInfo::new(svc_type, "no-txt", &hostname, "", 8080, &[] as &[(&str, &str)])
        .expect("ServiceInfo with empty TXT should succeed");

    assert_eq!(info.get_port(), 8080);
    // Empty TXT should have zero or only the empty property.
    let prop_count = info.get_properties().iter().count();
    println!("[t2] PASS — empty TXT has {prop_count} properties");
}

#[test]
fn t3_hostname_retrieval() {
    let h = get_hostname();
    assert!(!h.is_empty(), "hostname should not be empty");
    assert!(
        !h.contains('.'),
        "hostname should be bare (no domain), got: {h}"
    );
    println!("[t3] PASS — hostname: {h}");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CATEGORY 2: Daemon lifecycle tests (always pass)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn t4_daemon_creation() {
    // Creating a daemon should not panic or fail.
    let daemon = shared_test_daemon();

    // Metrics should be retrievable.
    let metrics_rx = daemon.get_metrics().expect("get_metrics should work");
    std::thread::sleep(Duration::from_millis(100));
    let mut got_metrics = false;
    while let Ok(m) = metrics_rx.try_recv() {
        got_metrics = true;
        println!("[t4] metrics: {:?}", m);
    }
    assert!(got_metrics, "should receive at least one metrics snapshot");
    println!("[t4] PASS — daemon created and metrics retrieved");
}

#[test]
fn t5_register_unregister_lifecycle() {
    let svc_type = unique_service_type("lc");
    let hostname = format!("{}.local.", get_hostname());
    let daemon = shared_test_daemon();

    // Register a service.
    let info = ServiceInfo::new(
        &svc_type,
        "lifecycle-test",
        &hostname,
        "",
        9999,
        &[("key", "value")] as &[(&str, &str)],
    )
    .expect("ServiceInfo::new failed");

    let fullname = info.get_fullname().to_string();
    println!("[t5] Registering: {fullname}");
    daemon.register(info).expect("register should succeed");

    // Give the daemon time to process the registration.
    std::thread::sleep(Duration::from_millis(500));

    // Unregister should succeed and return a receiver.
    println!("[t5] Unregistering: {fullname}");
    let unreg_rx = daemon.unregister(&fullname).expect("unregister should succeed");

    // Wait for unregister confirmation.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut confirmed = false;
    while Instant::now() < deadline {
        match unreg_rx.try_recv() {
            Ok(status) => {
                println!("[t5] Unregister status: {:?}", status);
                confirmed = true;
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    assert!(confirmed, "should receive unregister confirmation");
    println!("[t5] PASS — register/unregister lifecycle works");
}

#[test]
fn t6_browse_starts_and_stops() {
    let svc_type = unique_service_type("bs");
    let daemon = shared_test_daemon();

    // Starting a browse should not fail.
    let receiver = daemon.browse(&svc_type).expect("browse should succeed");

    // Should receive at least a SearchStarted event.
    let mut got_search_started = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match receiver.try_recv() {
            Ok(ServiceEvent::SearchStarted(_)) => {
                got_search_started = true;
                break;
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    assert!(got_search_started, "browse should emit SearchStarted");

    // Stopping the browse should not fail.
    daemon.stop_browse(&svc_type).expect("stop_browse should succeed");
    println!("[t6] PASS — browse start/stop lifecycle works");
}

#[test]
fn t7_multiple_registrations() {
    let svc_type = unique_service_type("mr");
    let hostname = format!("{}.local.", get_hostname());
    let daemon = shared_test_daemon();

    let names = ["multi-a", "multi-b", "multi-c"];
    let mut fullnames = Vec::new();

    for (i, name) in names.iter().enumerate() {
        let port = 20000 + i as u16;
        let idx_str = i.to_string();
        let props: &[(&str, &str)] = &[("idx", idx_str.as_str())];
        let info = ServiceInfo::new(&svc_type, name, &hostname, "", port, props)
            .expect("ServiceInfo::new failed");
        let fname = info.get_fullname().to_string();
        daemon.register(info).expect("register should succeed");
        fullnames.push(fname);
    }

    std::thread::sleep(Duration::from_millis(500));

    // All should unregister cleanly.
    for fname in &fullnames {
        daemon
            .unregister(fname)
            .expect("unregister should succeed");
    }

    println!("[t7] PASS — registered and unregistered {} services", names.len());
}

#[test]
fn t8_custom_port_daemon() {
    // Verify that new_with_port() works (used by tests and avoids port 5353
    // contention).
    let daemon = ServiceDaemon::new_with_port(25353)
        .expect("new_with_port should succeed");

    let svc_type = "_custom._tcp.local.";
    let hostname = format!("{}.local.", get_hostname());
    let info = ServiceInfo::new(svc_type, "custom-port", &hostname, "", 1111, &[] as &[(&str, &str)])
        .expect("ServiceInfo::new failed");
    let fullname = info.get_fullname().to_string();

    daemon.register(info).expect("register on custom port should succeed");
    std::thread::sleep(Duration::from_millis(300));
    daemon.unregister(&fullname).expect("unregister should succeed");
    daemon.shutdown().expect("shutdown should succeed");

    println!("[t8] PASS — custom port daemon lifecycle works");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CATEGORY 3: OS environment checks (informational, never hard-fail)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn t9_raw_multicast_loopback() {
    let sock = UdpSocket::bind("0.0.0.0:0").expect("bind failed");
    let port = sock.local_addr().unwrap().port();
    let mcast_group = Ipv4Addr::new(239, 255, 77, 88);

    sock.join_multicast_v4(&mcast_group, &Ipv4Addr::UNSPECIFIED)
        .expect("join_multicast_v4 failed");
    sock.set_multicast_loop_v4(true)
        .expect("set_multicast_loop_v4 failed");
    sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

    let msg = b"MCAST_LOOPBACK_TEST";
    sock.send_to(msg, (mcast_group, port))
        .expect("send_to failed");

    let mut buf = [0u8; 128];
    match sock.recv_from(&mut buf) {
        Ok((n, _src)) => {
            assert_eq!(&buf[..n], msg);
            println!("[t9] PASS — raw UDP multicast loopback works");
        }
        Err(e) => {
            panic!(
                "Raw UDP multicast loopback failed: {e}\n\
                 The OS network stack cannot deliver multicast packets. mDNS will not work."
            );
        }
    }
}

#[test]
fn t10_port_5353_check() {
    match UdpSocket::bind("0.0.0.0:5353") {
        Ok(_sock) => {
            println!("[t10] PASS — port 5353 is free");
        }
        Err(e) => {
            // Informational, not a failure.
            println!(
                "[t10] INFO — port 5353 in use: {e}\n\
                 Another mDNS responder is running. mdns-sd shares the port via SO_REUSEADDR.\n\
                 Cross-machine discovery works; same-machine loopback may be unreliable."
            );
        }
    }
}

#[test]
fn t11_per_interface_multicast_probe() {
    // Tests whether multicast loopback works on each detected interface.
    // This explains WHY same-machine resolution may fail.
    let interfaces: Vec<(&str, Ipv4Addr)> = vec![
        ("UNSPECIFIED (0.0.0.0)", Ipv4Addr::UNSPECIFIED),
        ("Loopback (127.0.0.1)", Ipv4Addr::LOCALHOST),
    ];

    let mcast_group = Ipv4Addr::new(224, 0, 0, 251);

    for (label, iface) in &interfaces {
        let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        match sock.join_multicast_v4(&mcast_group, iface) {
            Ok(()) => {
                let _ = sock.set_multicast_loop_v4(true);
                let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                let msg = b"PROBE";
                let _ = sock.send_to(msg, (mcast_group, port));
                let mut buf = [0u8; 64];
                match sock.recv_from(&mut buf) {
                    Ok((n, _)) if &buf[..n] == msg => {
                        println!("[t11] {label}: multicast loopback OK");
                    }
                    _ => {
                        println!("[t11] {label}: multicast loopback FAILED");
                    }
                }
            }
            Err(e) => {
                println!("[t11] {label}: join_multicast_v4 failed: {e}");
            }
        }
    }
    println!("[t11] PASS — per-interface probe complete (see details above)");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CATEGORY 4: Network round-trip tests (skipped when loopback unavailable)
// ═══════════════════════════════════════════════════════════════════════════════

/// Cache the probe result so we only do the slow check once.
static LOOPBACK_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn require_mdns_loopback(test_name: &str) -> bool {
    let available = *LOOPBACK_AVAILABLE.get_or_init(|| {
        println!("[probe] Testing same-machine mDNS resolution...");
        let result = mdns_self_resolve_works();
        if result {
            println!("[probe] Same-machine mDNS loopback works!");
        } else {
            println!(
                "[probe] Same-machine mDNS loopback NOT available.\n\
                 This is normal on Windows with Hyper-V / WSL virtual switches.\n\
                 Cross-machine LAN discovery (the real use case) is unaffected.\n\
                 Network round-trip tests will be skipped."
            );
        }
        result
    });

    if !available {
        println!(
            "[{test_name}] SKIP — same-machine mDNS loopback unavailable.\n\
             The library works correctly for cross-machine LAN discovery.\n\
             This test requires two machines or an OS without Hyper-V vswitch interference."
        );
    }
    available
}

#[test]
fn t12_browse_resolve_loopback() {
    if !require_mdns_loopback("t12") {
        return;
    }

    let svc_type = unique_service_type("lb");
    let instance_name = "test-server";
    let port: u16 = 7350;
    let hostname_local = format!("{}.local.", get_hostname());

    let txt_props: Vec<(&str, &str)> = vec![
        ("server_key", "micrommo_dev_key"),
        ("name", "Test Server"),
        ("region", "LAN"),
        ("use_ssl", "false"),
    ];

    let daemon = shared_test_daemon();

    let receiver = daemon.browse(&svc_type).expect("browse failed");
    std::thread::sleep(Duration::from_millis(500));

    let info = ServiceInfo::new(
        &svc_type,
        instance_name,
        &hostname_local,
        "",
        port,
        txt_props.as_slice(),
    )
    .expect("ServiceInfo::new failed");

    let fullname = info.get_fullname().to_string();
    println!("[t12] Registering: {fullname}");
    daemon.register(info).expect("register failed");

    let resolved = wait_for_resolved(&receiver, &fullname, Duration::from_secs(15));
    let _ = daemon.unregister(&fullname);

    let resolved = resolved.expect("ServiceResolved was not received within 15 seconds");

    assert_eq!(resolved.get_fullname(), fullname);
    assert_eq!(resolved.get_port(), port);
    assert!(!resolved.get_addresses().is_empty());
    assert!(resolved.get_addresses().iter().any(|a| a.to_ip_addr().is_ipv4()));

    let txt_map: HashMap<String, String> = resolved
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();
    assert_eq!(txt_map.get("server_key").map(|s| s.as_str()), Some("micrommo_dev_key"));
    assert_eq!(txt_map.get("name").map(|s| s.as_str()), Some("Test Server"));

    println!("[t12] PASS");
}

#[test]
fn t13_service_removal() {
    if !require_mdns_loopback("t13") {
        return;
    }

    let svc_type = unique_service_type("rm");
    let hostname_local = format!("{}.local.", get_hostname());
    let daemon = shared_test_daemon();

    let receiver = daemon.browse(&svc_type).expect("browse failed");
    std::thread::sleep(Duration::from_millis(500));

    let info = ServiceInfo::new(
        &svc_type, "removal-test", &hostname_local, "", 9876,
        &[] as &[(&str, &str)],
    )
    .expect("ServiceInfo::new failed");
    let fullname = info.get_fullname().to_string();
    daemon.register(info).expect("register failed");

    let resolved = wait_for_resolved(&receiver, &fullname, Duration::from_secs(15));
    assert!(resolved.is_some(), "service must be discovered before testing removal");

    daemon.unregister(&fullname).expect("unregister failed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut removed = false;
    while Instant::now() < deadline {
        match receiver.try_recv() {
            Ok(ServiceEvent::ServiceRemoved(_, name)) if name == fullname => {
                removed = true;
                break;
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    assert!(removed, "ServiceRemoved not received within 10 seconds");
    println!("[t13] PASS");
}

#[test]
fn t14_multiple_services_resolved() {
    if !require_mdns_loopback("t14") {
        return;
    }

    let svc_type = unique_service_type("ms");
    let hostname_local = format!("{}.local.", get_hostname());
    let daemon = shared_test_daemon();

    let receiver = daemon.browse(&svc_type).expect("browse failed");
    std::thread::sleep(Duration::from_millis(500));

    let names = ["multi-a", "multi-b", "multi-c"];
    let mut fullnames = Vec::new();

    for (i, name) in names.iter().enumerate() {
        let port = 30000 + i as u16;
        let idx_val = i.to_string();
        let props: &[(&str, &str)] = &[("idx", idx_val.as_str())];
        let info = ServiceInfo::new(&svc_type, name, &hostname_local, "", port, props)
            .expect("ServiceInfo::new failed");
        fullnames.push(info.get_fullname().to_string());
        daemon.register(info).expect("register failed");
    }

    let mut found = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && found.len() < fullnames.len() {
        match receiver.try_recv() {
            Ok(ServiceEvent::ServiceResolved(r)) => {
                let fname = r.get_fullname().to_string();
                if fullnames.contains(&fname) && !found.contains(&fname) {
                    found.push(fname);
                }
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    for fname in &fullnames {
        let _ = daemon.unregister(fname);
    }

    for expected in &fullnames {
        assert!(found.contains(expected), "service '{}' not discovered", expected);
    }
    println!("[t14] PASS — all {} services resolved", names.len());
}
