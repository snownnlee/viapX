// 目录恢复子模块
// 将已迁移目录恢复到原路径，按还原模式决定处理方式：
//   仅链接模式：只创建/修复原路径的链接，数据始终留在目标盘
//   保留模式：数据复制回原路径，目标副本保留
//   移动模式：数据复制回原路径并删除目标副本

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::migration::cleanup::{cleanup_leftover_backups, remove_directory_robust};
use crate::migration::copy_engine::{build_copy_plan, copy_dir};
use crate::migration::links::{
    create_directory_link, remove_directory_link, rollback_restore_link, verify_directory_link,
};
use crate::migration::{emit_progress, format_bytes};
use crate::models::RestoreMode;

/// 还原实际执行的动作，调用方据此组织提示文案
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RestoreAction {
    /// 原路径链接已正确指向目标，无需改动
    LinkVerified,
    /// 原路径链接缺失或指向别处，已重新创建
    LinkCreated,
    /// 数据已复制回原路径
    DataRestored,
}

impl RestoreAction {
    /// 是否只动了链接（数据仍在目标盘，迁移关系未变）
    pub(crate) fn is_link_only(self) -> bool {
        matches!(self, RestoreAction::LinkVerified | RestoreAction::LinkCreated)
    }
}

pub(crate) struct RestoreDirectoryResult {
    pub(crate) action: RestoreAction,
    /// 已复制回原路径的字节数；仅链接模式为 0
    pub(crate) restored_size: u64,
    /// 目标副本清理失败时不影响数据完整性，但需要提示调用方
    pub(crate) cleanup_warning: Option<String>,
    /// 目标副本是否仍保留（仅链接模式与保留模式为 true）
    pub(crate) target_kept: bool,
}

pub(crate) fn restore_directory_with_progress(
    original_path: &Path,
    target_path: &Path,
    task_id: &str,
    app_handle: &tauri::AppHandle,
    // 仅应用恢复需要补沙箱读取权限；用户文件夹恢复时传 false，避免权限扩散
    apply_app_container_acl: bool,
    mode: RestoreMode,
) -> Result<RestoreDirectoryResult, String> {
    if mode == RestoreMode::LinkOnly {
        // 只修链：没有复制过程，进度仅用于让界面从检查走到完成
        emit_progress(app_handle, task_id, 10.0, "linking", "正在校验目录链接...", 0, 0);
        let action = restore_link_only(original_path, target_path)?;
        emit_progress(app_handle, task_id, 100.0, "done", "目录链接已就绪", 0, 0);
        return Ok(RestoreDirectoryResult {
            action,
            restored_size: 0,
            cleanup_warning: None,
            target_kept: true,
        });
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let reporter = |step: &str, percent: f64, message: &str, copied: u64, total: u64| {
        emit_progress(app_handle, task_id, percent, step, message, copied, total);
    };
    let restore_plan = build_copy_plan(target_path, original_path, &cancel_flag, &reporter)?;
    let total_size = restore_plan.total_size;

    let original_parent = original_path
        .parent()
        .ok_or("无法获取原路径的父目录")?;
    crate::utils::check_disk_space_for_restore(original_parent, total_size)?;

    emit_progress(app_handle, task_id, 9.0, "linking", "正在移除原目录链接...", 0, total_size);
    if crate::utils::is_junction(original_path) {
        fs::remove_dir(original_path)
            .map_err(|e| format!("删除目录链接失败: {}", e))?;
    } else if original_path.exists() {
        return Err(format!(
            "原路径 {} 已存在且不是目录链接，拒绝恢复以保护数据。",
            original_path.display()
        ));
    }

    fs::create_dir_all(original_path)
        .map_err(|e| format!("创建原路径目录失败 {}: {}", original_path.display(), e))?;

    if let Err(e) = copy_dir(restore_plan, &cancel_flag, &reporter) {
        let rollback = rollback_restore_link(original_path, target_path);
        return Err(format!("恢复复制失败：{}\n{}", e, rollback));
    }

    emit_progress(app_handle, task_id, 90.0, "verifying", "正在校验恢复完整性...", total_size, total_size);
    let restored_size = crate::utils::get_dir_size_safe(original_path);
    let tolerance = (total_size as f64 * 0.01) as u64 + 1024 * 1024;
    if (restored_size as i64 - total_size as i64).abs() > tolerance as i64 {
        let rollback = rollback_restore_link(original_path, target_path);
        return Err(format!(
            "恢复完整性校验失败：预期 {}，实际 {}。\n{}",
            format_bytes(total_size),
            format_bytes(restored_size),
            rollback
        ));
    }

    // 恢复同样走文件复制：若原安装位置本来就在非系统盘（首次安装就没进 C 盘），
    // 复制出来的目录同样缺 ALL APPLICATION PACKAGES，需要一并补。
    // 失败只记录，不阻断恢复 —— 数据已经完整写回，回滚代价更大。
    if apply_app_container_acl {
        if let Err(e) = crate::migration::ensure_app_container_access(original_path) {
            log_warn!("migration", "恢复后目录权限补全失败 {}: {}", original_path.display(), e);
        }
    }

    let keep_target = mode == RestoreMode::KeepData;
    let cleanup_warning = if keep_target {
        log_warn!(
            "migration",
            "保留模式：数据已恢复到 {}，目标副本保留在 {}",
            original_path.display(),
            target_path.display()
        );
        None
    } else if target_path.exists() {
        emit_progress(app_handle, task_id, 96.0, "linking", "正在清理目标副本...", restored_size, total_size);
        remove_directory_robust(target_path)
            .err()
            .map(|e| format!("恢复已完成，但目标副本清理失败：{}", e))
    } else {
        None
    };

    // 还原成功后清理历史残留备份（迁移时备份清理失败遗留的多余副本），
    // 仍被占用的文件标记为重启后自动删除
    if let Some(parent) = original_path.parent() {
        cleanup_leftover_backups(parent);
    }

    emit_progress(app_handle, task_id, 100.0, "done", "恢复完成", restored_size, total_size);
    Ok(RestoreDirectoryResult {
        action: RestoreAction::DataRestored,
        restored_size,
        cleanup_warning,
        target_kept: keep_target,
    })
}

/// 仅链接模式：只保证原路径是指向目标的目录链接，绝不改动目标数据
///
/// 原路径已是真实目录时不做任何操作并明确拒绝 —— 那里面可能有应用更新后
/// 重新写入的新数据，静默删除或改名都超出"仅修链接"的授权范围。
pub(crate) fn restore_link_only(
    original_path: &Path,
    target_path: &Path,
) -> Result<RestoreAction, String> {
    if !target_path.is_dir() {
        return Err(format!(
            "目标数据不存在，无法按仅链接模式恢复：{}",
            target_path.display()
        ));
    }

    if crate::utils::is_junction(original_path) {
        return match verify_directory_link(original_path, target_path) {
            // 链接本身不含数据，指向错误时删掉重建是安全的
            Ok(()) => Ok(RestoreAction::LinkVerified),
            Err(reason) => {
                log_warn!(
                    "migration",
                    "原路径链接指向异常，按仅链接模式重建：{}（{}）",
                    original_path.display(),
                    reason
                );
                remove_directory_link(original_path)?;
                create_directory_link(target_path, original_path)?;
                verify_directory_link(original_path, target_path)?;
                Ok(RestoreAction::LinkCreated)
            }
        };
    }

    if original_path.exists() {
        return Err(format!(
            "原路径 {} 已经是真实目录（含数据），仅链接模式不会改动它。\n\n\
             请先把该目录的内容合并到目标位置或移走后重试；\n\
             需要直接把数据搬回来时，可在顶部菜单切换到「保留模式」或「移动模式」。",
            original_path.display()
        ));
    }

    if let Some(parent) = original_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建原路径父目录失败 {}: {}", parent.display(), e))?;
        }
    }
    create_directory_link(target_path, original_path)?;
    verify_directory_link(original_path, target_path)?;
    Ok(RestoreAction::LinkCreated)
}

/// 按模式与实际动作组织还原结果文案，三处还原入口共用
pub(crate) fn build_restore_message(
    subject: &str,
    original: &str,
    target: &str,
    mode: RestoreMode,
    action: RestoreAction,
    restored_size: u64,
    cleanup_warning: Option<&str>,
) -> String {
    let mut message = match action {
        RestoreAction::LinkVerified => format!(
            "{subject} 的目录链接已确认有效（仅链接模式）\n\n{} → {}\n\n数据仍保留在目标盘，迁移关系不变。",
            original, target
        ),
        RestoreAction::LinkCreated => format!(
            "{subject} 已按仅链接模式重建目录链接\n\n{} → {}\n\n数据仍保留在目标盘，迁移关系不变。",
            original, target
        ),
        RestoreAction::DataRestored => format!(
            "{subject} 已从 {} 恢复到 {}（{}，{}）",
            target,
            original,
            format_bytes(restored_size),
            mode.label()
        ),
    };

    if action == RestoreAction::DataRestored && mode == RestoreMode::KeepData {
        message.push_str(&format!(
            "\n\n目标副本按保留模式留在原处：{}",
            target
        ));
    }
    if let Some(warning) = cleanup_warning {
        message.push_str(&format!("\n\n{}", warning));
    }
    message
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("viap-restore-{tag}-{suffix}"));
        fs::create_dir_all(&dir).expect("创建测试目录失败");
        dir
    }

    fn make_target(root: &Path, name: &str) -> std::path::PathBuf {
        let target = root.join(name);
        fs::create_dir_all(&target).expect("创建目标目录失败");
        fs::write(target.join("payload.txt"), b"data").expect("写入测试文件失败");
        target
    }

    #[test]
    fn link_only_creates_link_when_original_missing() {
        let root = temp_root("create");
        let target = make_target(&root, "target");
        let original = root.join("original");

        let action = restore_link_only(&original, &target).expect("应创建链接");

        assert_eq!(action, RestoreAction::LinkCreated);
        assert!(crate::utils::is_junction(&original));
        assert!(original.join("payload.txt").is_file(), "链接应能访问目标数据");
        assert!(target.join("payload.txt").is_file(), "目标数据不得被移动");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn link_only_reuses_valid_link() {
        let root = temp_root("reuse");
        let target = make_target(&root, "target");
        let original = root.join("original");
        junction::create(&target, &original).expect("创建目录联接失败");

        let action = restore_link_only(&original, &target).expect("应复用链接");

        assert_eq!(action, RestoreAction::LinkVerified);
        assert!(original.join("payload.txt").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn link_only_rebuilds_link_pointing_elsewhere() {
        let root = temp_root("rebuild");
        let real_target = make_target(&root, "real-target");
        let stale_target = make_target(&root, "stale-target");
        let original = root.join("original");
        junction::create(&stale_target, &original).expect("创建目录联接失败");

        let action = restore_link_only(&original, &real_target).expect("应重建链接");

        assert_eq!(action, RestoreAction::LinkCreated);
        verify_directory_link(&original, &real_target).expect("链接应指向正确目标");
        assert!(original.join("payload.txt").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn link_only_refuses_real_directory_without_touching_it() {
        let root = temp_root("refuse");
        let target = make_target(&root, "target");
        let original = root.join("original");
        fs::create_dir_all(&original).expect("创建原路径目录失败");
        fs::write(original.join("newer.txt"), b"app updated data").expect("写入测试文件失败");

        let error = restore_link_only(&original, &target).expect_err("真实目录必须拒绝");

        assert!(error.contains("真实目录"), "错误应说明原因: {error}");
        assert!(original.join("newer.txt").is_file(), "原目录内容不得被改动");
        assert!(!crate::utils::is_junction(&original));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn link_only_refuses_missing_target() {
        let root = temp_root("missing");
        let original = root.join("original");

        let error = restore_link_only(&original, &root.join("no-such-target"))
            .expect_err("目标不存在必须拒绝");

        assert!(error.contains("目标数据不存在"), "错误应说明原因: {error}");
        assert!(!original.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
