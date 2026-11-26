// 迁移记录盘符修正
//
// 记录里写死绝对路径，数据盘盘符一变（换盘、重装后盘序变化）全部失效。
// 这里提供"旧盘符 → 新盘符"的预览与批量改写：迁移记录、绿色软件兜底元数据、
// 自定义文件夹条目一起修正，并把原路径上的目录联接重建到新位置。

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::data_dir::{ensure_data_dir, load_custom_folders, save_custom_folders};
use super::history::{get_history_file_path, load_history, save_history};
use super::migrated_app_metadata::{load_all, save_all};
use crate::migration::{create_directory_link, remove_directory_link, verify_directory_link};
use crate::utils;

/// 单条路径改写预览
#[derive(Serialize)]
pub struct DriveRepairItem {
    pub id: String,
    pub app_name: String,
    pub from: String,
    pub to: String,
    /// 改写后的新路径当前是否存在（不存在仍然可以改，只是要提醒用户）
    pub new_exists: bool,
}

/// 需要重建的原路径链接
#[derive(Serialize)]
pub struct DriveLinkItem {
    pub id: String,
    pub app_name: String,
    pub original_path: String,
    pub from_target: String,
    pub to_target: String,
}

#[derive(Serialize)]
pub struct DriveRepairDrives {
    /// 记录中出现过的盘符（大写）
    pub record_drives: Vec<String>,
    /// 当前系统存在的盘符（大写）
    pub existing_drives: Vec<String>,
}

#[derive(Serialize)]
pub struct DriveRepairPreview {
    pub old_drive: String,
    pub new_drive: String,
    pub new_drive_exists: bool,
    pub target_items: Vec<DriveRepairItem>,
    pub original_items: Vec<DriveRepairItem>,
    pub metadata_count: u32,
    pub custom_folder_count: u32,
    pub links: Vec<DriveLinkItem>,
}

#[derive(Serialize)]
pub struct DriveRepairResult {
    pub records_updated: u32,
    pub metadata_updated: u32,
    pub custom_folders_updated: u32,
    pub links_rebuilt: u32,
    pub link_failures: Vec<String>,
    pub backup_path: Option<String>,
}

fn normalize_drive(input: &str) -> Result<char, String> {
    let trimmed = input.trim();
    let letter = trimmed.trim_end_matches([':', '\\', '/']).trim();
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase()),
        _ => Err(format!("盘符格式无效：{}（只接受单个字母，例如 F）", input)),
    }
}

/// 路径以旧盘符开头时替换成新盘符，否则返回 None
fn remap_drive(path: &str, old: char, new: char) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() >= 2
        && bytes[1] == b':'
        && (bytes[0] as char).to_ascii_uppercase() == old
    {
        return Some(format!("{}{}", new, &path[1..]));
    }
    None
}

fn drive_of(path: &str) -> Option<char> {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

fn existing_drives() -> Vec<String> {
    ('A'..='Z')
        .filter(|letter| Path::new(&format!("{}:\\", letter)).exists())
        .map(|letter| letter.to_string())
        .collect()
}

fn custom_folders_file() -> PathBuf {
    utils::custom_folders_path(&ensure_data_dir())
}

/// 列出可选盘符：记录里用到的 + 当前系统存在的
#[tauri::command]
pub fn list_drive_repair_drives() -> DriveRepairDrives {
    let mut record_drives: Vec<String> = Vec::new();
    for record in load_history().records {
        for drive in [drive_of(&record.target_path), drive_of(&record.original_path)] {
            if let Some(letter) = drive {
                let value = letter.to_string();
                if !record_drives.contains(&value) {
                    record_drives.push(value);
                }
            }
        }
    }
    record_drives.sort();
    DriveRepairDrives {
        record_drives,
        existing_drives: existing_drives(),
    }
}

/// 只读预览：列出会被改写的路径与需要重建的链接
#[tauri::command]
pub fn preview_drive_repair(
    old_drive: String,
    new_drive: String,
) -> Result<DriveRepairPreview, String> {
    let old = normalize_drive(&old_drive)?;
    let new = normalize_drive(&new_drive)?;
    if old == new {
        return Err("新旧盘符相同，无需修正".to_string());
    }

    let mut target_items = Vec::new();
    let mut original_items = Vec::new();
    let mut links = Vec::new();

    for record in load_history().records {
        if let Some(to) = remap_drive(&record.target_path, old, new) {
            target_items.push(DriveRepairItem {
                id: record.id.clone(),
                app_name: record.app_name.clone(),
                from: record.target_path.clone(),
                new_exists: Path::new(&to).exists(),
                to,
            });
        }
        if let Some(to) = remap_drive(&record.original_path, old, new) {
            original_items.push(DriveRepairItem {
                id: record.id.clone(),
                app_name: record.app_name.clone(),
                from: record.original_path.clone(),
                new_exists: Path::new(&to).exists(),
                to,
            });
        }
        // 原路径是联接、且当前指向旧盘 → 需要重建
        let original = Path::new(&record.original_path);
        if utils::is_junction(original) {
            if let Some(current) = utils::get_junction_target(original) {
                if let Some(to_target) = remap_drive(&current, old, new) {
                    links.push(DriveLinkItem {
                        id: record.id.clone(),
                        app_name: record.app_name.clone(),
                        original_path: record.original_path.clone(),
                        from_target: current,
                        to_target,
                    });
                }
            }
        }
    }

    let metadata_count = load_all()
        .iter()
        .filter(|entry| {
            remap_drive(&entry.target_path, old, new).is_some()
                || remap_drive(&entry.original_path, old, new).is_some()
        })
        .count() as u32;

    let custom_folder_count = load_custom_folders(&custom_folders_file())
        .iter()
        .filter(|folder| remap_drive(&folder.path, old, new).is_some())
        .count() as u32;

    Ok(DriveRepairPreview {
        old_drive: old.to_string(),
        new_drive: new.to_string(),
        new_drive_exists: Path::new(&format!("{}:\\", new)).exists(),
        target_items,
        original_items,
        metadata_count,
        custom_folder_count,
        links,
    })
}

/// 执行修正：改写三处路径并按需重建原路径链接
#[tauri::command]
pub fn apply_drive_repair(
    old_drive: String,
    new_drive: String,
) -> Result<DriveRepairResult, String> {
    let old = normalize_drive(&old_drive)?;
    let new = normalize_drive(&new_drive)?;
    if old == new {
        return Err("新旧盘符相同，无需修正".to_string());
    }

    // 1. 迁移记录
    let mut history = load_history();
    let mut records_updated = 0u32;
    let mut link_jobs: Vec<(String, String, String)> = Vec::new();
    for record in history.records.iter_mut() {
        let mut changed = false;
        if let Some(path) = remap_drive(&record.target_path, old, new) {
            record.target_path = path;
            changed = true;
        }
        if let Some(path) = remap_drive(&record.original_path, old, new) {
            record.original_path = path;
            changed = true;
        }
        if changed {
            records_updated += 1;
        }

        // 链接任务按磁盘上的实际指向判断：盘符变了但联接还指着旧位置
        let original = Path::new(&record.original_path);
        if utils::is_junction(original) {
            if let Some(current) = utils::get_junction_target(original) {
                if let Some(target) = remap_drive(&current, old, new) {
                    link_jobs.push((
                        record.app_name.clone(),
                        record.original_path.clone(),
                        target,
                    ));
                }
            }
        }
    }
    if records_updated > 0 {
        save_history(&history)?;
    }

    // 2. 绿色软件兜底元数据
    let mut apps = load_all();
    let mut metadata_updated = 0u32;
    for entry in apps.iter_mut() {
        let mut changed = false;
        if let Some(path) = remap_drive(&entry.target_path, old, new) {
            entry.target_path = path;
            changed = true;
        }
        if let Some(path) = remap_drive(&entry.original_path, old, new) {
            entry.original_path = path;
            changed = true;
        }
        if changed {
            metadata_updated += 1;
        }
    }
    if metadata_updated > 0 {
        save_all(&apps)?;
    }

    // 3. 自定义文件夹
    let folders_path = custom_folders_file();
    let mut folders = load_custom_folders(&folders_path);
    let mut custom_folders_updated = 0u32;
    for folder in folders.iter_mut() {
        if let Some(path) = remap_drive(&folder.path, old, new) {
            folder.path = path;
            custom_folders_updated += 1;
        }
    }
    if custom_folders_updated > 0 {
        save_custom_folders(&folders_path, &folders)?;
    }

    // 4. 重建原路径链接：新目标不存在时只记录不动手，避免把可用链接删成空
    let mut links_rebuilt = 0u32;
    let mut link_failures = Vec::new();
    for (name, original, new_target) in link_jobs {
        let link = Path::new(&original);
        let target = Path::new(&new_target);
        if !target.exists() {
            link_failures.push(format!(
                "{}：新位置不存在 {}（记录已修正，链接保持原样）",
                name, new_target
            ));
            continue;
        }
        let outcome = (|| -> Result<(), String> {
            remove_directory_link(link)?;
            create_directory_link(target, link)?;
            verify_directory_link(link, target)
        })();
        match outcome {
            Ok(()) => links_rebuilt += 1,
            Err(error) => link_failures.push(format!(
                "{}：重建失败 {}（原路径当前可能不是链接，可用「恢复记录」重新识别）",
                name, error
            )),
        }
    }

    let backup = get_history_file_path().with_extension("json.bak");
    Ok(DriveRepairResult {
        records_updated,
        metadata_updated,
        custom_folders_updated,
        links_rebuilt,
        link_failures,
        backup_path: if backup.exists() {
            Some(backup.to_string_lossy().to_string())
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::remap_drive;

    #[test]
    fn remaps_only_matching_drive() {
        assert_eq!(
            remap_drive(r"F:\Viap\programs\App", 'F', 'G').as_deref(),
            Some(r"G:\Viap\programs\App")
        );
        assert_eq!(
            remap_drive(r"f:\Viap\App", 'F', 'G').as_deref(),
            Some(r"G:\Viap\App")
        );
        assert_eq!(remap_drive(r"C:\Program Files\App", 'F', 'G'), None);
        assert_eq!(remap_drive(r"\\server\share", 'F', 'G'), None);
        assert_eq!(remap_drive(r"", 'F', 'G'), None);
    }

    #[test]
    fn rejects_bad_drive_input() {
        assert!(super::normalize_drive("F").is_ok());
        assert!(super::normalize_drive("f:\\").is_ok());
        assert!(super::normalize_drive("FG").is_err());
        assert!(super::normalize_drive("").is_err());
        assert!(super::normalize_drive("1").is_err());
    }
}
