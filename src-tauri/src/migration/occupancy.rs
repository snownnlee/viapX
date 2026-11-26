// 文件占用检测子模块
// 以独占模式打开文件探测占用，用于迁移前预检

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
#[cfg(windows)]
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use rayon::prelude::*;
use serde::Serialize;

use super::copy_engine::{CopyPlan, ProgressReporter};

/// ERROR_SHARING_VIOLATION：其他句柄以不兼容方式持有该文件
#[cfg(windows)]
const ERROR_SHARING_VIOLATION: i32 = 32;
/// ERROR_ACCESS_DENIED：权限被拒（ACL 显式拒绝、加密等），与"被占用"语义不同
#[cfg(windows)]
const ERROR_ACCESS_DENIED: i32 = 5;

/// 瞬时共享冲突的重试次数：杀软实时扫描、搜索索引、云同步会在毫秒级释放句柄
#[cfg(windows)]
const BUSY_RETRIES: u32 = 3;
/// 重试间隔：三次累计约 60 ms，真被占用时每个文件多耗这点时间可以接受
#[cfg(windows)]
const BUSY_RETRY_DELAY_MS: u64 = 20;

/// 结果中保留的明细条目上限；计数不受此限制，超出部分只计数不记录
const MAX_ENTRIES: usize = 200;
/// 占用检测日志保留份数
const LOG_KEEP: usize = 20;

/// 打开失败的原因分类
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LockKind {
    /// 其他句柄以不兼容方式持有：关闭程序、注销或重启有效
    Busy,
    /// 权限被拒：关闭程序与重启均无效，需要调整权限或以管理员身份运行
    Denied,
}

/// 单条检测结果
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LockedFile {
    /// 相对源根目录的路径
    pub(crate) path: String,
    pub(crate) kind: LockKind,
    /// 位于可重建目录内：被占用也只是缓存失效，可容忍
    pub(crate) rebuildable: bool,
    /// 打开失败的操作系统错误码
    pub(crate) errno: i32,
    /// 文件只读位（只读不阻止读取，仅作排查信息记录）
    pub(crate) readonly: bool,
    /// Windows 文件属性位，仅作排查信息记录
    pub(crate) attributes: u32,
}

impl LockedFile {
    /// 是否必须阻塞迁移
    ///
    /// 权限拒绝意味着复制根本读不到内容；可重建目录内的忙句柄即使真被占用，
    /// 用户也宁可丢弃重建，不应因此卡死整个迁移。
    pub(crate) fn blocks_migration(&self) -> bool {
        match self.kind {
            LockKind::Denied => true,
            LockKind::Busy => !self.rebuildable,
        }
    }
}

/// 检测结果
///
/// `entries` 只用于展示与落盘（有上限），是否阻塞一律看计数字段。
#[derive(Default, Serialize)]
pub(crate) struct LockReport {
    pub(crate) source_root: String,
    pub(crate) total_files: u64,
    pub(crate) entries: Vec<LockedFile>,
    pub(crate) busy: u64,
    pub(crate) denied: u64,
    pub(crate) rebuildable: u64,
}

impl LockReport {
    /// 必须阻塞迁移的条目总数（不受展示上限影响）
    pub(crate) fn blocking_count(&self) -> u64 {
        self.denied + self.busy.saturating_sub(self.rebuildable)
    }

    /// 分类型取出明细，供文案分组展示
    pub(crate) fn split(&self) -> (Vec<&LockedFile>, Vec<&LockedFile>) {
        self.entries
            .iter()
            .partition(|file| file.kind == LockKind::Denied)
    }
}

/// 检测结果：取消与"无占用"必须区分开，避免取消被当成占用误报
pub(crate) enum LockCheckOutcome {
    Cancelled,
    Report(LockReport),
}

/// 检测目录内文件是否被其他进程独占持有
///
/// 原理：以独占模式（FILE_SHARE_NONE）尝试打开每个文件。
/// 失败按错误码分流：
/// - 32 ERROR_SHARING_VIOLATION：真有其他句柄，重试仍失败才算占用
/// - 5  ERROR_ACCESS_DENIED：权限问题，重试无意义，直接归类为权限受限
///
/// 适用场景：
/// - 数据目录：检测真实写锁（浏览器缓存等）
/// - 应用目录：检测被 explorer/系统服务加载的 DLL（如 shell extension），
///   这类文件虽不阻塞复制（读共享），但会导致迁移后的备份目录清理失败
///
/// 注意：此方法检测不到应用本体——exe 被内存映射时不阻塞独占打开，
/// 应用本体需用进程 exe 路径匹配检测，两者互补使用。
///
/// 文件清单直接复用复制计划（`CopyPlan`），避免对同一棵树重复遍历。
///
/// 命中结果会写入数据目录下的 `logs/lockcheck-<时间戳>.json`（全量计数 + 明细），
/// UI 只展示前若干条，判断误报时可据此复查。
#[cfg(windows)]
pub(crate) fn check_file_locks(
    plan: &CopyPlan,
    cancel_flag: &Arc<AtomicBool>,
    progress: ProgressReporter<'_>,
) -> LockCheckOutcome {
    if cancel_flag.load(Ordering::Relaxed) {
        return LockCheckOutcome::Cancelled;
    }

    let source_root = &plan.source_root;
    let total_files = plan.file_list.len() as u64;
    let entries: Mutex<Vec<LockedFile>> = Mutex::new(Vec::new());
    let busy_count = AtomicU64::new(0);
    let denied_count = AtomicU64::new(0);
    let rebuildable_count = AtomicU64::new(0);
    let checked_count = AtomicU64::new(0);
    let last_report = AtomicU64::new(0);

    // 高风险扩展名优先：被加载的 DLL / 运行中的 exe 才会阻塞复制与备份清理，
    // 先查它们可以在真正有占用时立刻失败，而不是把整棵树扫完才报错
    let (risky, others): (Vec<&PathBuf>, Vec<&PathBuf>) = plan
        .file_list
        .iter()
        .map(|(relative_path, _)| relative_path)
        .partition(|relative_path| has_risky_extension(relative_path));

    let report_progress = |checked: u64| {
        // 每 2000 个文件上报一次，避免高频事件拖慢检测本身
        let previous = last_report.load(Ordering::Relaxed);
        if checked < previous + 2000 && checked != total_files {
            return;
        }
        if last_report
            .compare_exchange(previous, checked, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let percent = (checked as f64 / total_files.max(1) as f64 * 9.0).min(9.0);
        progress(
            "checking",
            percent,
            &format!("正在检测文件占用 {}/{}", checked, total_files),
            0,
            0,
        );
    };

    for group in [risky, others] {
        group.par_iter().for_each(|relative_path| {
            if cancel_flag.load(Ordering::Relaxed) {
                return;
            }
            // 每 200 个文件检查一次取消标志（并行下仍保留节流，避免无谓打开）
            let count = checked_count.fetch_add(1, Ordering::Relaxed) + 1;
            if count % 200 == 0 {
                if cancel_flag.load(Ordering::Relaxed) {
                    return;
                }
                report_progress(count);
            }

            let absolute = source_root.join(relative_path);
            let Some(errno) = probe_exclusive(&absolute) else {
                return;
            };
            let kind = match errno {
                ERROR_SHARING_VIOLATION => LockKind::Busy,
                ERROR_ACCESS_DENIED => LockKind::Denied,
                // 文件消失等竞态忽略，不视为占用
                _ => return,
            };
            let rebuildable = kind == LockKind::Busy && is_rebuildable_path(relative_path);
            match kind {
                LockKind::Busy => {
                    busy_count.fetch_add(1, Ordering::Relaxed);
                    if rebuildable {
                        rebuildable_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
                LockKind::Denied => {
                    denied_count.fetch_add(1, Ordering::Relaxed);
                }
            }

            // 明细有上限但继续扫完全部文件：否则可能漏掉后面才出现的权限拒绝
            let mut list = entries.lock().unwrap();
            if list.len() < MAX_ENTRIES {
                let (readonly, attributes) = file_flags(&absolute);
                list.push(LockedFile {
                    path: relative_path.to_string_lossy().to_string(),
                    kind,
                    rebuildable,
                    errno,
                    readonly,
                    attributes,
                });
            }
        });
    }
    report_progress(total_files);

    if cancel_flag.load(Ordering::Relaxed) {
        return LockCheckOutcome::Cancelled;
    }

    let mut entries = entries.into_inner().unwrap();
    // 并行收集顺序不定，按路径排序让提示与日志稳定
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    let report = LockReport {
        source_root: source_root.to_string_lossy().to_string(),
        total_files,
        entries,
        busy: busy_count.load(Ordering::Relaxed),
        denied: denied_count.load(Ordering::Relaxed),
        rebuildable: rebuildable_count.load(Ordering::Relaxed),
    };
    // 单测会构造占用样本，不该往用户数据目录写日志
    if !report.entries.is_empty() && !cfg!(test) {
        write_report_log(&report);
    }
    LockCheckOutcome::Report(report)
}

/// 非 Windows 平台回退：无占用检测，返回空报告
#[cfg(not(windows))]
pub(crate) fn check_file_locks(
    _plan: &CopyPlan,
    _cancel_flag: &Arc<AtomicBool>,
    _progress: ProgressReporter<'_>,
) -> LockCheckOutcome {
    LockCheckOutcome::Report(LockReport::default())
}

/// 独占打开探测一次文件
///
/// 只对 `32` 重试：瞬时句柄多在 60 ms 内释放，而 `5` 是权限问题重试无意义。
/// 返回 `None` 表示可独占打开（未被占用）。
#[cfg(windows)]
fn probe_exclusive(path: &Path) -> Option<i32> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut attempt = 0;
    loop {
        attempt += 1;
        // FILE_SHARE_NONE = 0，独占打开。若其他进程持有该文件句柄则失败。
        match fs::OpenOptions::new().read(true).share_mode(0).open(path) {
            Ok(_) => return None,
            Err(error) => {
                let errno = error.raw_os_error().unwrap_or(0);
                if errno != ERROR_SHARING_VIOLATION || attempt >= BUSY_RETRIES {
                    return Some(errno);
                }
                std::thread::sleep(std::time::Duration::from_millis(BUSY_RETRY_DELAY_MS));
            }
        }
    }
}

/// 读取只读位与 Windows 文件属性用于日志排查；读取失败按默认值记录
#[cfg(windows)]
fn file_flags(path: &Path) -> (bool, u32) {
    use std::os::windows::fs::MetadataExt;

    match fs::metadata(path) {
        Ok(metadata) => (metadata.permissions().readonly(), metadata.file_attributes()),
        Err(_) => (false, 0),
    }
}

/// 判断是否为「容易被加载 / 正在运行」的文件类型
///
/// 这类文件被占用时复制会失败、备份也删不掉，优先检测它们。
#[cfg(windows)]
fn has_risky_extension(relative_path: &Path) -> bool {
    const RISKY: &[&str] = &[
        "exe", "dll", "sys", "node", "so", "dylib", "ocx", "cpl", "scr", "drv", "pdb", "tmp",
    ];
    relative_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| RISKY.iter().any(|risky| extension.eq_ignore_ascii_case(risky)))
        .unwrap_or(false)
}

/// 判断路径是否位于可重建目录内
///
/// 这些目录由包管理器或构建工具生成，被占用时用户宁可丢弃重建，
/// 因此其中的忙句柄降级为提示，不阻塞迁移。
#[cfg(windows)]
fn is_rebuildable_path(relative_path: &Path) -> bool {
    const REBUILDABLE: &[&str] = &[
        "node_modules",
        ".pnpm",
        ".yarn",
        "target",
        "dist",
        ".next",
        ".nuxt",
        ".turbo",
        "__pycache__",
    ];
    relative_path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|name| REBUILDABLE.iter().any(|dir| name.eq_ignore_ascii_case(dir)))
            .unwrap_or(false)
    })
}

/// 把检测结果落盘到 `logs/lockcheck-<时间戳>.json`
///
/// 该模块此前不写任何日志，误报时只能靠手工逐文件复测定位。
/// 写日志失败不影响迁移，仅记录原因。
fn write_report_log(report: &LockReport) {
    let log_dir = crate::storage::data_dir::ensure_data_dir().join("logs");
    if let Err(error) = fs::create_dir_all(&log_dir) {
        log_warn!("migration", "创建占用检测日志目录失败 {}: {}", log_dir.display(), error);
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let path = log_dir.join(format!("lockcheck-{}.json", timestamp));
    let json = match serde_json::to_string_pretty(report) {
        Ok(json) => json,
        Err(error) => {
            log_warn!("migration", "序列化占用检测结果失败: {}", error);
            return;
        }
    };
    if let Err(error) = fs::write(&path, json) {
        log_warn!("migration", "写入占用检测日志失败 {}: {}", path.display(), error);
        return;
    }
    prune_logs(&log_dir);
}

/// 只保留最近的若干份检测日志，避免长期使用后无限增长
fn prune_logs(log_dir: &Path) {
    let Ok(entries) = fs::read_dir(log_dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("lockcheck-") && name.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    if logs.len() <= LOG_KEEP {
        return;
    }
    logs.sort();
    for path in &logs[..logs.len() - LOG_KEEP] {
        if let Err(error) = fs::remove_file(path) {
            log_warn!("migration", "清理旧占用检测日志失败 {}: {}", path.display(), error);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    /// 构造只含指定文件的复制计划，避免为测试再走一遍扫描
    fn plan_with(root: &Path, files: &[&str]) -> CopyPlan {
        CopyPlan {
            source_root: root.to_path_buf(),
            target_root: root.join("__target__"),
            file_list: files
                .iter()
                .map(|name| (PathBuf::from(name), 1_u64))
                .collect(),
            dir_list: Vec::new(),
            link_list: Vec::new(),
            has_executable: false,
            total_size: files.len() as u64,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("viap-lock-{tag}-{suffix}"));
        fs::create_dir_all(&dir).expect("创建测试目录失败");
        dir
    }

    fn run_check(plan: &CopyPlan) -> LockReport {
        let cancel = Arc::new(AtomicBool::new(false));
        let reporter = |_: &str, _: f64, _: &str, _: u64, _: u64| {};
        match check_file_locks(plan, &cancel, &reporter) {
            LockCheckOutcome::Cancelled => panic!("未取消却返回取消"),
            LockCheckOutcome::Report(report) => report,
        }
    }

    #[test]
    fn free_file_is_not_reported() {
        let root = temp_dir("free");
        fs::write(root.join("data.txt"), b"payload").expect("写入测试文件失败");

        let report = run_check(&plan_with(&root, &["data.txt"]));

        assert!(report.entries.is_empty());
        assert_eq!(report.blocking_count(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn busy_file_is_reported_as_busy() {
        let root = temp_dir("busy");
        let file = root.join("locked.dat");
        fs::write(&file, b"payload").expect("写入测试文件失败");
        // 同进程保持一个不共享的句柄，探测必须返回 32
        let _handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .expect("独占打开测试文件失败");

        let report = run_check(&plan_with(&root, &["locked.dat"]));

        assert_eq!(report.busy, 1);
        assert_eq!(report.denied, 0);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].kind, LockKind::Busy);
        assert!(report.entries[0].blocks_migration());
        drop(_handle);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rebuildable_busy_file_does_not_block() {
        let root = temp_dir("rebuildable");
        let nested = root.join("node_modules").join("pkg");
        fs::create_dir_all(&nested).expect("创建测试目录失败");
        let file = nested.join("index.js");
        fs::write(&file, b"payload").expect("写入测试文件失败");
        let _handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .expect("独占打开测试文件失败");

        let report = run_check(&plan_with(&root, &["node_modules/pkg/index.js"]));

        assert_eq!(report.busy, 1);
        assert_eq!(report.rebuildable, 1);
        assert_eq!(report.blocking_count(), 0, "可重建目录内的忙句柄不应阻塞迁移");
        assert!(!report.entries[0].blocks_migration());
        drop(_handle);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rebuildable_detection_matches_directory_names() {
        assert!(is_rebuildable_path(Path::new("app/node_modules/pkg/a.js")));
        assert!(is_rebuildable_path(Path::new("project/Target/debug/a.pdb")));
        assert!(is_rebuildable_path(Path::new("py/__pycache__/a.pyc")));
        assert!(!is_rebuildable_path(Path::new("target_data/keep.bin")));
        assert!(!is_rebuildable_path(Path::new("data/a.bin")));
    }

    /// 需要能修改 ACL：无权限或非 Windows 环境下直接跳过，不误报失败
    #[test]
    fn denied_file_is_classified_as_denied() {
        let root = temp_dir("denied");
        let file = root.join("denied.dat");
        fs::write(&file, b"payload").expect("写入测试文件失败");

        let applied = std::process::Command::new("icacls")
            .arg(&file)
            .args(["/deny", "BUILTIN\\Users:(R)"])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !applied {
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let report = run_check(&plan_with(&root, &["denied.dat"]));

        // 还原 ACL 后再断言，避免断言失败时把 DENY 留在临时文件上
        let _ = std::process::Command::new("icacls")
            .arg(&file)
            .args(["/remove:d", "BUILTIN\\Users"])
            .output();

        assert_eq!(report.denied, 1);
        assert_eq!(report.entries[0].kind, LockKind::Denied);
        assert!(report.entries[0].blocks_migration());
        let _ = fs::remove_dir_all(&root);
    }
}
