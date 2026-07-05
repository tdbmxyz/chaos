//! Host metrics of the machine running chaos-server (the glance
//! "server-stats" widget). Everything sysinfo reads is cheap (/proc and
//! statfs), but it is still blocking I/O, so it runs on the blocking pool.

use chaos_domain::{DiskUsage, ServerStats, WidgetData};
use sysinfo::{Disks, MemoryRefreshKind, RefreshKind, System};

pub async fn collect(mounts: Vec<String>) -> Result<WidgetData, String> {
    tokio::task::spawn_blocking(move || collect_sync(&mounts))
        .await
        .map_err(|e| format!("stats task failed: {e}"))
}

fn collect_sync(mounts: &[String]) -> WidgetData {
    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    let load = System::load_average();

    let mut disks: Vec<DiskUsage> = Disks::new_with_refreshed_list()
        .iter()
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| DiskUsage {
            mount: disk.mount_point().to_string_lossy().into_owned(),
            total_bytes: disk.total_space(),
            used_bytes: disk.total_space().saturating_sub(disk.available_space()),
        })
        .filter(|disk| mounts.is_empty() || mounts.contains(&disk.mount))
        .collect();
    // Btrfs subvolumes and bind mounts show up as duplicates of the same
    // filesystem; keep one entry per mount point.
    disks.sort_by(|a, b| a.mount.cmp(&b.mount));
    disks.dedup_by(|a, b| a.mount == b.mount);

    WidgetData::ServerStats(ServerStats {
        hostname: System::host_name(),
        uptime_secs: System::uptime(),
        load_avg: [load.one, load.five, load.fifteen],
        mem_total_bytes: sys.total_memory(),
        mem_used_bytes: sys.used_memory(),
        disks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collects_plausible_stats() {
        let WidgetData::ServerStats(stats) = collect(Vec::new()).await.expect("stats collect")
        else {
            panic!("wrong widget data kind");
        };
        assert!(stats.mem_total_bytes > 0);
        assert!(stats.mem_used_bytes <= stats.mem_total_bytes);
        assert!(stats.uptime_secs > 0);
    }

    #[tokio::test]
    async fn mount_filter_limits_disks() {
        let WidgetData::ServerStats(stats) = collect(vec!["/".into()]).await.expect("stats") else {
            panic!("wrong widget data kind");
        };
        assert!(stats.disks.iter().all(|d| d.mount == "/"));
    }
}
