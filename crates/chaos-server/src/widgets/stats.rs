//! Host metrics of the machine running chaos-server (the glance
//! "server-stats" widget). Everything sysinfo reads is cheap (/proc and
//! statfs), but it is still blocking I/O, so it runs on the blocking pool.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use chaos_domain::{DiskUsage, ServerStats, StatPoint, WidgetData};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

/// Rolling CPU/memory samples, written by the sampler task and attached to
/// every stats payload. Plain mutex: touched twice a minute.
pub type History = Arc<Mutex<VecDeque<StatPoint>>>;

/// Sample CPU and memory every [`ServerStats::HISTORY_INTERVAL_SECS`] into a
/// bounded ring buffer. CPU utilisation is measured between consecutive
/// refreshes, so the sampler's own cadence is the measurement window.
pub fn spawn_sampler() -> History {
    let history: History = Arc::default();
    let handle = history.clone();
    tokio::spawn(async move {
        let refresh = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
            .with_memory(MemoryRefreshKind::nothing().with_ram());
        let mut sys = System::new_with_specifics(refresh);
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(
            ServerStats::HISTORY_INTERVAL_SECS,
        ));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await; // first tick fires immediately; skip the 0% sample
        loop {
            tick.tick().await;
            sys.refresh_specifics(refresh);
            let point = StatPoint {
                cpu_pct: sys.global_cpu_usage(),
                mem_used_bytes: sys.used_memory(),
            };
            let mut buf = handle.lock().expect("stats history lock");
            if buf.len() >= ServerStats::HISTORY_LEN {
                buf.pop_front();
            }
            buf.push_back(point);
        }
    });
    history
}

pub async fn collect(mounts: Vec<String>, history: Vec<StatPoint>) -> Result<WidgetData, String> {
    tokio::task::spawn_blocking(move || collect_sync(&mounts, history))
        .await
        .map_err(|e| format!("stats task failed: {e}"))
}

/// One mounted filesystem as statvfs reports it.
struct RawMount {
    /// Device / dataset name ("dpool/persist/media", "/dev/nvme0n1p2").
    device: String,
    fs: String,
    mount: String,
    total: u64,
    avail: u64,
}

impl RawMount {
    /// Space consumed by this filesystem itself.
    fn used(&self) -> u64 {
        self.total.saturating_sub(self.avail)
    }
}

fn collect_sync(mounts: &[String], history: Vec<StatPoint>) -> WidgetData {
    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    let load = System::load_average();

    let raw: Vec<RawMount> = Disks::new_with_refreshed_list()
        .iter()
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| RawMount {
            device: disk.name().to_string_lossy().into_owned(),
            fs: disk.file_system().to_string_lossy().into_owned(),
            mount: disk.mount_point().to_string_lossy().into_owned(),
            total: disk.total_space(),
            avail: disk.available_space(),
        })
        .collect();

    WidgetData::ServerStats(ServerStats {
        hostname: System::host_name(),
        uptime_secs: System::uptime(),
        load_avg: [load.one, load.five, load.fifteen],
        mem_total_bytes: sys.total_memory(),
        mem_used_bytes: sys.used_memory(),
        disks: disk_usage(raw, mounts),
        history,
    })
}

/// Turn raw mounts into displayable rows.
///
/// ZFS needs special handling: statvfs on a dataset reports
/// `total = the dataset's own usage + pool free space`, so every child of a
/// pool shows a misleading near-empty "disk" the size of whatever happens to
/// be free. Instead, datasets are aggregated into one row per pool
/// (`used = Σ own usage`, `total = used + free`, free being shared) — and a
/// dataset explicitly named in the `mounts` filter is shown as its own usage
/// against the pool's real capacity.
///
/// Everything else keeps statvfs numbers; bind mounts and btrfs subvolumes
/// of one filesystem are deduped by mount point.
///
/// Known approximation: snapshot/reservation usage and unmounted datasets
/// are invisible to statvfs, so pool totals run a few % under `zpool list`.
/// Good enough for a dashboard meter without shelling out to zfs.
fn disk_usage(raw: Vec<RawMount>, mounts: &[String]) -> Vec<DiskUsage> {
    // (pool name) -> (Σ used over datasets, min avail seen)
    let mut pools: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut rows: Vec<DiskUsage> = Vec::new();
    let mut seen_mounts: Vec<String> = Vec::new();
    // A dataset can be mounted at several paths (zfs mountpoint + bind
    // mount, like /dpool/persist/media + /data/media); count each once.
    let mut seen_datasets: Vec<&str> = Vec::new();

    for m in &raw {
        if m.fs != "zfs" || seen_datasets.contains(&m.device.as_str()) {
            continue;
        }
        seen_datasets.push(&m.device);
        let pool = m.device.split('/').next().unwrap_or(&m.device).to_string();
        let entry = pools.entry(pool).or_insert((0, u64::MAX));
        entry.0 += m.used();
        entry.1 = entry.1.min(m.avail);
    }

    for m in raw {
        if seen_mounts.contains(&m.mount) {
            continue; // bind mounts / subvolume duplicates
        }
        seen_mounts.push(m.mount.clone());

        if m.fs == "zfs" {
            // Only explicitly requested datasets get their own row.
            if !mounts.contains(&m.mount) {
                continue;
            }
            let pool = m.device.split('/').next().unwrap_or(&m.device);
            let (pool_used, pool_avail) = pools[pool];
            rows.push(DiskUsage {
                total_bytes: pool_used.saturating_add(pool_avail),
                used_bytes: m.used(),
                mount: m.mount,
            });
        } else if mounts.is_empty() || mounts.contains(&m.mount) {
            rows.push(DiskUsage {
                total_bytes: m.total,
                used_bytes: m.used(),
                mount: m.mount,
            });
        }
    }

    // Pool rows: always shown unless a filter exists and leaves them out.
    for (pool, (used, avail)) in pools {
        if !mounts.is_empty() && !mounts.contains(&pool) {
            continue;
        }
        rows.push(DiskUsage {
            mount: pool,
            total_bytes: used.saturating_add(avail),
            used_bytes: used,
        });
    }

    rows.sort_by(|a, b| a.mount.cmp(&b.mount));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zfs(device: &str, mount: &str, used: u64, avail: u64) -> RawMount {
        RawMount {
            device: device.into(),
            fs: "zfs".into(),
            mount: mount.into(),
            total: used + avail,
            avail,
        }
    }

    fn ext4(mount: &str, total: u64, avail: u64) -> RawMount {
        RawMount {
            device: "/dev/sda1".into(),
            fs: "ext4".into(),
            mount: mount.into(),
            total,
            avail,
        }
    }

    const T: u64 = 1 << 40;

    #[test]
    fn zfs_datasets_collapse_into_pool_totals() {
        let raw = vec![
            zfs("dpool/media", "/data/media", 4 * T, T),
            zfs("dpool/keeps", "/data/keeps", T, T),
            zfs("dpool", "/dpool", 0, T),
        ];
        let rows = disk_usage(raw, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mount, "dpool");
        assert_eq!(rows[0].used_bytes, 5 * T);
        // capacity = every dataset's own usage + the shared free space
        assert_eq!(rows[0].total_bytes, 6 * T);
    }

    #[test]
    fn multiply_mounted_datasets_count_once() {
        // zeus mounts every dataset at its zfs mountpoint AND under /data.
        let raw = vec![
            zfs("dpool/media", "/dpool/persist/media", 4 * T, T),
            zfs("dpool/media", "/data/media", 4 * T, T),
            zfs("dpool", "/dpool", 0, T),
        ];
        let rows = disk_usage(raw, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].used_bytes, 4 * T);
        assert_eq!(rows[0].total_bytes, 5 * T);
    }

    #[test]
    fn filtered_dataset_shows_own_usage_against_pool_capacity() {
        let raw = vec![
            zfs("dpool/media", "/data/media", 4 * T, T),
            zfs("dpool/keeps", "/data/keeps", T, T),
        ];
        let rows = disk_usage(raw, &["/data/media".to_string(), "dpool".to_string()]);
        assert_eq!(rows.len(), 2);
        let media = rows.iter().find(|r| r.mount == "/data/media").unwrap();
        assert_eq!(media.used_bytes, 4 * T);
        assert_eq!(media.total_bytes, 6 * T);
        let pool = rows.iter().find(|r| r.mount == "dpool").unwrap();
        assert_eq!(pool.used_bytes, 5 * T);
    }

    #[test]
    fn non_zfs_mounts_keep_statvfs_numbers_and_dedup() {
        let raw = vec![ext4("/", 100, 40), ext4("/", 100, 40), ext4("/boot", 10, 5)];
        let rows = disk_usage(raw, &[]);
        assert_eq!(rows.len(), 2);
        let root = rows.iter().find(|r| r.mount == "/").unwrap();
        assert_eq!(root.total_bytes, 100);
        assert_eq!(root.used_bytes, 60);
    }

    #[tokio::test]
    async fn collects_plausible_stats() {
        let WidgetData::ServerStats(stats) = collect(Vec::new(), Vec::new())
            .await
            .expect("stats collect")
        else {
            panic!("wrong widget data kind");
        };
        assert!(stats.mem_total_bytes > 0);
        assert!(stats.mem_used_bytes <= stats.mem_total_bytes);
        assert!(stats.uptime_secs > 0);
    }

    #[tokio::test]
    async fn mount_filter_limits_disks() {
        let WidgetData::ServerStats(stats) =
            collect(vec!["/".into()], Vec::new()).await.expect("stats")
        else {
            panic!("wrong widget data kind");
        };
        assert!(stats.disks.iter().all(|d| d.mount == "/"));
    }
}
