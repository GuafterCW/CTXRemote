//! What the host's computer is: system, hardware, disks and network, for the
//! viewer's info panel (`ViewerMsg::GetSystemInfo`).

use ctxremote_proto::session::{DiskInfo, NetworkInfo, SystemInfo};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

/// Takes a moment (disks, network); call it off the async threads.
pub fn gather() -> SystemInfo {
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()).with_cpu(CpuRefreshKind::nothing()),
    );
    let cpu = system.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
    let disks = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| DiskInfo {
            mount: d.mount_point().to_string_lossy().into_owned(),
            label: d.name().to_string_lossy().into_owned(),
            total: d.total_space(),
            free: d.available_space(),
        })
        .collect();
    let mut networks: Vec<NetworkInfo> = Networks::new_with_refreshed_list()
        .iter()
        .filter_map(|(name, data)| {
            let addresses: Vec<String> = data
                .ip_networks()
                .iter()
                .filter(|n| !n.addr.is_loopback() && !is_link_local(&n.addr))
                .map(|n| n.addr.to_string())
                .collect();
            let mac = data.mac_address();
            // Interfaces without an address or hardware (tunnels, loopback) say little.
            (!addresses.is_empty() && mac.0 != [0; 6]).then(|| NetworkInfo { name: name.clone(), mac: mac.to_string(), addresses })
        })
        .collect();
    networks.sort_by(|a, b| a.name.cmp(&b.name));
    SystemInfo {
        hostname: System::host_name().unwrap_or_default(),
        user: whoami::username(),
        os: System::long_os_version().unwrap_or_else(|| whoami::distro()),
        os_build: System::kernel_version().unwrap_or_default(),
        model: model(),
        cpu,
        cores: System::physical_core_count().unwrap_or(system.cpus().len()) as u32,
        memory_total: system.total_memory(),
        memory_used: system.used_memory(),
        uptime_secs: System::uptime(),
        disks,
        networks,
        app_version: crate::update::VERSION.to_string(),
    }
}

fn is_link_local(addr: &std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    }
}

/// Maker and model from the firmware tables Windows keeps in the registry.
#[cfg(windows)]
fn model() -> String {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let read = |name: PCWSTR| -> Option<String> {
        let key = w!(r"HARDWARE\DESCRIPTION\System\BIOS");
        let mut buf = [0u16; 256];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: the buffer and its length in bytes match.
        unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
                .ok()
                .ok()?;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]).trim().to_string()).filter(|s| !s.is_empty())
    };
    match (read(w!("SystemManufacturer")), read(w!("SystemProductName"))) {
        (Some(maker), Some(product)) => format!("{maker} {product}"),
        (maker, product) => maker.or(product).unwrap_or_default(),
    }
}

#[cfg(not(windows))]
fn model() -> String {
    std::fs::read_to_string("/sys/class/dmi/id/product_name").map(|s| s.trim().to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn gathers_something() {
        let info = super::gather();
        assert!(!info.hostname.is_empty());
        assert!(info.memory_total > 0);
        assert!(info.memory_used <= info.memory_total);
        assert!(!info.app_version.is_empty());
    }
}
