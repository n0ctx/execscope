use crate::model::Status;
use serde::Serialize;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
use {crate::safe, std::path::Path};

const DNS_TARGET: &str = "example.com";
const HTTPS_TARGET: &str = "https://example.com/";
const DNS_TIMEOUT: Duration = Duration::from_millis(700);
const HTTPS_TIMEOUT: Duration = Duration::from_millis(1350);

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Passive {
    pub(crate) status: Status,
    pub(crate) proxy: &'static str,
    pub(crate) dns: &'static str,
    pub(crate) vpn: &'static str,
    process_proxy: bool,
    system_proxy: Option<bool>,
    dns_source: &'static str,
}

pub(crate) fn passive() -> Passive {
    let process_proxy = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()));
    let (system_proxy, dns, dns_source) = local_config();
    let proxy = if process_proxy || system_proxy == Some(true) {
        "configured"
    } else if system_proxy == Some(false) {
        "none"
    } else {
        "unknown"
    };
    Passive {
        status: if proxy == "unknown" || dns == "unknown" {
            Status::Unknown
        } else {
            Status::Available
        },
        proxy,
        dns,
        vpn: vpn_indicator(),
        process_proxy,
        system_proxy,
        dns_source,
    }
}

#[cfg(target_os = "linux")]
fn local_config() -> (Option<bool>, &'static str, &'static str) {
    let dns = safe::read(Path::new("/etc/resolv.conf"), 32 * 1024)
        .is_some_and(|s| resolver_configured(&s));
    let proxy = safe::read(Path::new("/etc/environment"), 32 * 1024).map(|s| {
        s.lines().any(|line| {
            line.split_once('=').is_some_and(|(key, value)| {
                matches!(
                    key.trim().to_ascii_lowercase().as_str(),
                    "http_proxy" | "https_proxy" | "all_proxy"
                ) && !value.trim().trim_matches(['\'', '"']).is_empty()
            })
        })
    });
    (
        proxy,
        if dns { "configured" } else { "unknown" },
        "resolv_conf",
    )
}

#[cfg(any(target_os = "linux", test))]
fn resolver_configured(contents: &str) -> bool {
    contents.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some("nameserver")
            && fields.next().is_some_and(|ip| ip.parse::<IpAddr>().is_ok())
    })
}

#[cfg(target_os = "macos")]
fn local_config() -> (Option<bool>, &'static str, &'static str) {
    use std::ffi::{c_char, c_void};
    type Cf = *const c_void;
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyProxies(store: Cf) -> Cf;
        fn SCDynamicStoreCopyValue(store: Cf, key: Cf) -> Cf;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: Cf, value: *const c_char, encoding: u32) -> Cf;
        fn CFDictionaryGetValue(dictionary: Cf, key: Cf) -> Cf;
        fn CFNumberGetTypeID() -> usize;
        fn CFGetTypeID(value: Cf) -> usize;
        fn CFNumberGetValue(number: Cf, kind: isize, result: *mut i32) -> u8;
        fn CFRelease(value: Cf);
    }
    let null = std::ptr::null();
    // CoreFoundation owns returned Copy/Create objects; borrowed dictionary values are not released.
    let (proxy, dns) = unsafe {
        let proxies = SCDynamicStoreCopyProxies(null);
        let proxy = if proxies.is_null() {
            None
        } else {
            let configured = [
                c"HTTPEnable",
                c"HTTPSEnable",
                c"SOCKSEnable",
                c"ProxyAutoConfigEnable",
                c"ProxyAutoDiscoveryEnable",
            ]
            .iter()
            .any(|name| {
                let key = CFStringCreateWithCString(null, name.as_ptr(), 0x08000100);
                if key.is_null() {
                    return false;
                }
                let val = CFDictionaryGetValue(proxies, key);
                CFRelease(key);
                let mut enabled: i32 = 0;
                !val.is_null()
                    && CFGetTypeID(val) == CFNumberGetTypeID()
                    && CFNumberGetValue(val, 9, &mut enabled) != 0
                    && enabled != 0
            });
            CFRelease(proxies);
            Some(configured)
        };
        let key =
            CFStringCreateWithCString(null, c"State:/Network/Global/DNS".as_ptr(), 0x08000100);
        let dns = if key.is_null() {
            false
        } else {
            let value = SCDynamicStoreCopyValue(null, key);
            CFRelease(key);
            if value.is_null() {
                false
            } else {
                CFRelease(value);
                true
            }
        };
        (proxy, dns)
    };
    (
        proxy,
        if dns { "configured" } else { "unknown" },
        "system_configuration",
    )
}

#[cfg(windows)]
fn local_config() -> (Option<bool>, &'static str, &'static str) {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }
    let internet = wide(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let mut value = 0u32;
    let mut bytes = 4u32;
    let manual = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            internet.as_ptr(),
            wide("ProxyEnable").as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut bytes,
        )
    };
    let mut size = 0u32;
    let pac = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            internet.as_ptr(),
            wide("AutoConfigURL").as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    let proxy = ((pac == 0 && size > 2) || (manual == 0 && value != 0)).then_some(true);
    let dns = wide(r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters");
    let dns_set = ["NameServer", "DhcpNameServer"].iter().any(|key| {
        let mut bytes = 0u32;
        (unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                dns.as_ptr(),
                wide(key).as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut bytes,
            )
        }) == 0
            && bytes > 2
    });
    (
        proxy,
        if dns_set { "configured" } else { "unknown" },
        "tcpip_global_registry",
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn local_config() -> (Option<bool>, &'static str, &'static str) {
    (None, "unknown", "unsupported")
}

fn vpn_indicator() -> &'static str {
    #[cfg(unix)]
    {
        // Names are only an indicator. Do not read interface addresses or expose names.
        unsafe {
            let list = libc::if_nameindex();
            if list.is_null() {
                return "unknown";
            }
            let mut result = "not_observed";
            for i in 0..128 {
                let entry = &*list.add(i);
                if entry.if_name.is_null() {
                    break;
                }
                let name = std::ffi::CStr::from_ptr(entry.if_name).to_bytes();
                if [b"tun".as_slice(), b"utun", b"tap", b"wg", b"tailscale"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                {
                    result = "tunnel_interface_observed";
                    break;
                }
            }
            libc::if_freenameindex(list);
            result
        }
    }
    #[cfg(not(unix))]
    {
        "unknown"
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Active {
    pub(crate) status: Status,
    pub(crate) dns: Check,
    pub(crate) https: Check,
    pub(crate) route: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Check {
    pub(crate) status: Status,
    target: &'static str,
    method: &'static str,
    elapsed_ms: u64,
    pub(crate) result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
}

pub(crate) async fn active(proxy_configured: bool) -> Active {
    active_with(
        tokio::net::lookup_host((DNS_TARGET, 443)),
        proxy_configured,
        |addresses| async move {
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(HTTPS_TIMEOUT)
                .resolve_to_addrs(DNS_TARGET, &addresses)
                .build()
                .map_err(|_| "client_unavailable")?;
            let response = client.head(HTTPS_TARGET).send().await.map_err(|e| {
                if e.is_timeout() {
                    "timed_out"
                } else {
                    "tls_or_connection_error"
                }
            })?;
            Ok(response.status().as_u16())
        },
    )
    .await
}

async fn active_with<I, D, H, F>(resolve: D, proxy_configured: bool, https: H) -> Active
where
    I: Iterator<Item = SocketAddr>,
    D: Future<Output = std::io::Result<I>>,
    H: FnOnce(Vec<SocketAddr>) -> F,
    F: Future<Output = Result<u16, &'static str>>,
{
    let started = Instant::now();
    let (addresses, status, result) = match tokio::time::timeout(DNS_TIMEOUT, resolve).await {
        Ok(Ok(addrs)) => {
            let addrs: Vec<_> = addrs
                .take(32)
                .filter(|addr| public_ip(addr.ip()))
                .take(8)
                .collect();
            if addrs.is_empty() {
                (addrs, Status::Unknown, "no_public_address")
            } else {
                (addrs, Status::Available, "resolved_public_address")
            }
        }
        Ok(Err(_)) => (vec![], Status::Unknown, "lookup_failed"),
        Err(_) => (vec![], Status::Timeout, "timed_out"),
    };
    let dns = Check {
        status,
        target: DNS_TARGET,
        method: "system_resolver",
        elapsed_ms: elapsed(started),
        result,
        http_status: None,
    };
    let started = Instant::now();
    let (status, result, http_status) = if proxy_configured {
        (Status::Unsupported, "skipped_configured_proxy", None)
    } else if addresses.is_empty() {
        (Status::Unknown, "skipped_no_safe_address", None)
    } else {
        match tokio::time::timeout(HTTPS_TIMEOUT, https(addresses)).await {
            Ok(Ok(code)) => (Status::Available, "http_response_received", Some(code)),
            Ok(Err("timed_out")) | Err(_) => (Status::Timeout, "timed_out", None),
            Ok(Err(reason)) => (Status::Unknown, reason, None),
        }
    };
    Active {
        status,
        dns,
        https: Check {
            status,
            target: HTTPS_TARGET,
            method: "HEAD",
            elapsed_ms: elapsed(started),
            result,
            http_status,
        },
        route: if proxy_configured {
            "https_not_attempted_configured_proxy"
        } else if http_status.is_some() || matches!(result, "timed_out" | "tls_or_connection_error")
        {
            "direct_no_application_proxy"
        } else {
            "https_not_attempted"
        },
    }
}

fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn public_ip(ip: IpAddr) -> bool {
    fn v4(ip: Ipv4Addr) -> bool {
        let [a, b, c, _] = ip.octets();
        !(ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_multicast()
            || ip.is_documentation()
            || a == 0
            || a >= 240
            || (a == 100 && (64..=127).contains(&b))
            || (a == 192 && b == 0 && c == 0)
            || (a == 198 && matches!(b, 18 | 19)))
    }
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] == 0xdb8 || s[1] < 0x200))
                && s[0] != 0x2002
                && s[0] != 0x3fff
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;

    #[test]
    fn config_is_presence_only() {
        assert!(resolver_configured(
            "nameserver 10.0.0.1 # proxy auth must not appear"
        ));
        assert!(!resolver_configured(
            "# nameserver 10.0.0.1\nnameserver anything"
        ));
    }

    #[tokio::test]
    async fn dns_failure_or_private_answer_never_starts_https() {
        for answer in [
            Err(std::io::ErrorKind::PermissionDenied.into()),
            Ok(vec!["198.18.0.72:443".parse().unwrap()].into_iter()),
        ] {
            let result = active_with(ready(answer), false, |_| async {
                panic!("HTTPS must not start")
            })
            .await;
            assert_eq!(result.https.result, "skipped_no_safe_address");
            assert_eq!(result.status, Status::Unknown);
        }
    }

    #[tokio::test]
    async fn configured_proxy_never_triggers_direct_https() {
        let result = active_with(
            ready(Ok(["93.184.216.34:443".parse().unwrap()].into_iter())),
            true,
            |_| async { panic!("proxy bypass is not allowed") },
        )
        .await;
        assert_eq!(result.https.status, Status::Unsupported);
        assert_eq!(result.https.result, "skipped_configured_proxy");
        assert_eq!(result.route, "https_not_attempted_configured_proxy");
    }

    #[tokio::test]
    async fn reachable_target_is_not_internet_status() {
        let result = active_with(
            ready(Ok(["93.184.216.34:443".parse().unwrap()].into_iter())),
            false,
            |_| async { Ok(503) },
        )
        .await;
        assert_eq!(result.https.http_status, Some(503));
        assert_eq!(result.https.status, Status::Available);
    }
}
