// 已迁移父目录判定
// 应用本身可能没有迁移记录，而是随某个被迁移的上级目录（如 DSpace）一起搬到了目标盘。

export interface MigratedParent {
  /** 父目录的原路径 */
  originalPath: string;
  /** 父目录当前指向的目标路径 */
  targetPath: string;
  /** 父目录名称（用于提示） */
  appName: string;
}

function normalize(path: string): string {
  return path.trim().replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();
}

/**
 * 找出路径所属的已迁移父目录（取最深的那个）
 *
 * 只匹配严格上级目录：路径本身有迁移记录时由调用方优先处理。
 */
export function findMigratedParent(
  path: string,
  parents: MigratedParent[],
): MigratedParent | undefined {
  const target = normalize(path);
  let matched: MigratedParent | undefined;
  for (const parent of parents) {
    const prefix = `${normalize(parent.originalPath)}\\`;
    if (!target.startsWith(prefix)) continue;
    if (!matched || parent.originalPath.length > matched.originalPath.length) {
      matched = parent;
    }
  }
  return matched;
}
