export interface TargetExistsConflict {
  isRetry: boolean;
  existingPath: string;
}

/**
 * 后端使用英文前缀作为迁移控制协议，前端必须先解析再展示，
 * 避免把 TARGET_EXISTS_RETRY 这类内部状态码暴露给普通用户。
 */
export function parseTargetExistsConflict(message: string): TargetExistsConflict | null {
  if (message.startsWith('TARGET_EXISTS_RETRY:')) {
    return {
      isRetry: true,
      existingPath: message.replace('TARGET_EXISTS_RETRY:', '').trim(),
    };
  }

  if (message.startsWith('TARGET_EXISTS:')) {
    return {
      isRetry: false,
      existingPath: message.replace('TARGET_EXISTS:', '').trim(),
    };
  }

  return null;
}

export interface LockWarning {
  /** 被检测的源目录 */
  source: string;
  /** 权限受限（ACL 拒绝或加密）：关闭程序与重启均无效 */
  denied: string[];
  /** 被其他程序独占打开：关闭程序、注销或重启有效 */
  busy: string[];
  /** 位于可重建目录（node_modules 等）内的占用条目数，可忽略 */
  rebuildable: number;
  /** 需要用户确认才能继续的条目总数（可能大于展示条数） */
  blockingTotal: number;
}

/** 后端载荷字段（snake_case），解析后统一转成 camelCase 使用 */
interface LockWarningWire {
  source?: unknown;
  denied?: unknown;
  busy?: unknown;
  rebuildable?: unknown;
  blocking_total?: unknown;
}

/**
 * 解析后端「文件占用/权限受限」预检协议。
 * 返回 null 表示不是该协议，解析失败按普通错误处理。
 */
export function parseLockWarning(message: string): LockWarning | null {
  const prefix = 'LOCK_WARN:';
  if (!message.startsWith(prefix)) return null;

  try {
    const payload = JSON.parse(message.slice(prefix.length)) as LockWarningWire;
    const toPaths = (value: unknown): string[] => Array.isArray(value)
      ? value.filter((item): item is string => typeof item === 'string')
      : [];
    const denied = toPaths(payload.denied);
    const busy = toPaths(payload.busy);
    return {
      source: typeof payload.source === 'string' ? payload.source : '',
      denied,
      busy,
      rebuildable: typeof payload.rebuildable === 'number' ? payload.rebuildable : 0,
      blockingTotal: typeof payload.blocking_total === 'number'
        ? payload.blocking_total
        : denied.length + busy.length,
    };
  } catch {
    return null;
  }
}

/** 每类最多展示的文件条数，其余按数量概括，避免弹窗过长 */
const LOCK_HINT_DISPLAY_LIMIT = 8;

function formatPathList(paths: string[]): string[] {
  const shown = paths.slice(0, LOCK_HINT_DISPLAY_LIMIT).map((path) => `  • ${path}`);
  if (paths.length > LOCK_HINT_DISPLAY_LIMIT) {
    shown.push(`  …等共 ${paths.length} 个`);
  }
  return shown;
}

/**
 * 把预检结果转成可确认的弹窗文案。
 * 两类问题处置方式完全不同（关程序 vs 调权限），必须分开列出。
 */
export function buildLockWarningPrompt(warning: LockWarning): string {
  const sections: string[] = [];

  if (warning.denied.length > 0) {
    sections.push([
      '【权限受限】这些文件没有读取权限（ACL 拒绝或加密），关闭程序与重启系统均无效：',
      ...formatPathList(warning.denied),
      '处理方式：以管理员身份运行本程序，或调整这些文件的权限后重试。',
    ].join('\n'));
  }

  if (warning.busy.length > 0) {
    sections.push([
      '【正在使用】这些文件被其他程序独占打开：',
      ...formatPathList(warning.busy),
      '处理方式：关闭正在使用它们的程序（浏览器、编辑器、游戏等）后重试，必要时注销或重启。',
    ].join('\n'));
  }

  if (warning.rebuildable > 0) {
    sections.push(`另有 ${warning.rebuildable} 个文件位于可重建目录（node_modules、target 等）内，已被视为可忽略。`);
  }

  return `目录中有 ${warning.blockingTotal} 个文件无法独占打开：\n\n${sections.join('\n\n')}\n\n仍要继续迁移吗？继续后这些文件可能在复制阶段失败。`;
}

/**
 * 解析「目标目录为空」确认协议，返回目标路径。
 * 空目录不再静默拒绝/清理，由前端弹窗让用户选择继续还是取消。
 */
export function parseTargetEmptyConfirm(message: string): string | null {
  const prefix = 'TARGET_EMPTY_CONFIRM:';
  if (!message.startsWith(prefix)) return null;
  return message.slice(prefix.length).trim();
}

/**
 * 将后端迁移内部协议和兜底错误统一转成中文用户提示。
 * 控制协议仍保留给流程判断使用，但任何进入 UI 的文本都应走这里。
 */
export function formatMigrationFailureMessage(message: string): string {
  const safeMessage = message.trim();
  if (!safeMessage) return '迁移失败：后端未返回具体原因，请查看日志后重试。';

  const targetConflict = parseTargetExistsConflict(safeMessage);
  if (targetConflict) {
    return targetConflict.isRetry
      ? `目标位置已存在同名目录：\n${targetConflict.existingPath}\n\n该目录可能是上次迁移未完成留下的残留，也可能是手动创建的数据。请确认该目录可覆盖后再重试。`
      : `目标位置已存在同名目录：\n${targetConflict.existingPath}\n\n为避免覆盖已有数据，已停止迁移。请更换迁移目录，或确认该目录可覆盖后重试。`;
  }

  if (safeMessage.startsWith('SOURCE_IS_LINK:')) {
    const payload = safeMessage.replace('SOURCE_IS_LINK:', '').trim();
    const [existingTarget = '未知位置', ...detailLines] = payload.split(/\r?\n/);
    const details = detailLines.join('\n').trim();
    return `该目录此前已经迁移过（当前指向 ${existingTarget}），不能重复迁移。\n\n`
      + (details || '再次迁移会把数据重复复制一份，旧副本仍留在原处占用空间。')
      + '\n\n如需改变位置，请先在「迁移记录」中还原该项目，再重新迁移。';
  }

  if (safeMessage.startsWith('JUNCTION_LOOP:')) {
    const targetPath = safeMessage.replace('JUNCTION_LOOP:', '').trim();
    return `检测到原路径仍是指向目标盘的目录链接，无法覆盖迁移。\n\n请先在「迁移记录」中恢复该项目，再重新迁移。\n\n目标位置：${targetPath}`;
  }

  if (safeMessage.startsWith('REQUIRES_WARNING_CONFIRM:')) {
    const detail = safeMessage.replace('REQUIRES_WARNING_CONFIRM:', '').trim();
    return `该目录属于高风险路径，需要通过风险确认后才能迁移。\n\n${detail}`;
  }

  const emptyTarget = parseTargetEmptyConfirm(safeMessage);
  if (emptyTarget !== null) {
    return `目标目录为空，没有可恢复的数据：\n${emptyTarget}\n\n如果是应用已被卸载或数据被清理，该记录不再需要还原。`;
  }

  const lockWarning = parseLockWarning(safeMessage);
  if (lockWarning) {
    return `目录中有 ${lockWarning.blockingTotal} 个文件无法独占打开（权限受限 ${lockWarning.denied.length} 个 / 被占用 ${lockWarning.busy.length} 个），已停止迁移。\n\n`
      + '请在重新发起迁移时按弹窗提示选择「忽略并继续」，或先关闭占用程序 / 调整权限后重试。';
  }

  if (safeMessage.startsWith('SYMLINK_FAILED_DATA_AT_TARGET:')) {
    const payload = safeMessage.replace('SYMLINK_FAILED_DATA_AT_TARGET:', '').trim();
    const [targetPath = '', ...detailLines] = payload.split(/\r?\n/);
    const details = detailLines.join('\n').trim();
    return details || `目录链接创建失败，数据已保留在目标位置：\n${targetPath}\n\n请确认目标数据完整后，再手动处理原路径或重试恢复。`;
  }

  return safeMessage;
}
