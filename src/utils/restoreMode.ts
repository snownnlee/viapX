// 还原模式定义与文案
// 与后端 models::RestoreMode 的序列化值保持一致（snake_case）

export type RestoreMode = 'link_only' | 'keep_data' | 'move';

export const DEFAULT_RESTORE_MODE: RestoreMode = 'link_only';

export interface RestoreModeMeta {
  key: RestoreMode;
  /** 模式全名，用于菜单项与确认弹窗 */
  label: string;
  /** 顶部按钮上的短名 */
  short: string;
  /** 菜单里的一行说明 */
  summary: string;
  /** 还原确认弹窗里的完整说明 */
  detail: string;
}

export const RESTORE_MODES: RestoreModeMeta[] = [
  {
    key: 'link_only',
    label: '仅链接模式',
    short: '仅链接',
    summary: '只创建或修复链接，数据留在目标盘',
    detail: '数据不动，只保证原路径是指向目标盘的目录链接：链接缺失就补建，指向不对就重建。\n原路径若已是含数据的真实目录，会拒绝操作并提示先手动处理。',
  },
  {
    key: 'keep_data',
    label: '保留模式',
    short: '保留',
    summary: '数据搬回原路径，目标副本保留',
    detail: '把数据复制回原路径，同时保留目标盘上的完整副本，占用双份空间，方便之后再次取用。\n原路径若已是含数据的真实目录，会拒绝操作。',
  },
  {
    key: 'move',
    label: '移动模式',
    short: '移动',
    summary: '数据搬回原路径，删除目标副本',
    detail: '把数据复制回原路径，校验通过后删除目标盘副本，与旧版本行为一致。\n原路径若已是含数据的真实目录，会拒绝操作。',
  },
];

export function isRestoreMode(value: unknown): value is RestoreMode {
  return value === 'link_only' || value === 'keep_data' || value === 'move';
}

export function normalizeRestoreMode(value: unknown): RestoreMode {
  return isRestoreMode(value) ? value : DEFAULT_RESTORE_MODE;
}

export function restoreModeMeta(mode: RestoreMode): RestoreModeMeta {
  return RESTORE_MODES.find((item) => item.key === mode) ?? RESTORE_MODES[0];
}

/**
 * 操作动词：仅链接模式下按钮实际是"修链"，数据不搬回，不能叫还原/恢复
 */
export function restoreActionLabel(mode: RestoreMode): string {
  return mode === 'link_only' ? '修复链接' : '还原';
}

/** 列表按钮悬停说明，直接写清当前模式会做什么 */
export function restoreActionHint(mode: RestoreMode, targetPath?: string): string {
  if (mode === 'link_only') {
    return `仅链接模式：只检查并修复原路径的目录链接，数据仍留在目标盘${targetPath ? `（${targetPath}）` : ''}`;
  }
  if (mode === 'keep_data') {
    return `保留模式：把数据搬回原路径，目标副本继续保留${targetPath ? `（${targetPath}）` : ''}`;
  }
  return `移动模式：把数据搬回原路径，并删除目标副本`;
}

/** 目标目录为空时的二次确认文案：先说清后果，再让用户决定继续还是取消 */
export function buildEmptyTargetWarning(mode: RestoreMode, targetPath: string): string {
  const consequence = mode === 'link_only'
    ? '原路径会变成一个指向这个空目录的目录链接'
    : mode === 'keep_data'
      ? '原路径会变成空目录，目标盘这个空目录同时保留'
      : '原路径会变成空目录，随后目标盘这个空目录被删除';
  return `目标目录为空，里面没有任何文件：\n${targetPath}\n\n`
    + `可能是应用已被卸载，或数据被手动清理。继续后${consequence}。\n\n是否继续？`;
}

export interface RestoreConfirmCopy {
  title: string;
  message: string;
  okLabel: string;
}

/**
 * 还原确认弹窗文案：按用户选择"只提示当前模式、不可临时改"，
 * 需要换模式时指引到顶部菜单。
 *
 * @param verb 应用用「还原」、文件夹用「恢复」；仅链接模式统一改成「修复链接」
 */
export function buildRestoreConfirm(
  mode: RestoreMode,
  subject: string,
  verb: '还原' | '恢复' = '还原',
): RestoreConfirmCopy {
  const meta = restoreModeMeta(mode);
  const action = mode === 'link_only' ? '修复链接' : verb;
  return {
    title: `确认${action}`,
    message: `将按「${meta.label}」${action}${subject}。\n\n${meta.detail}\n\n如需更换模式，请先用顶部菜单的「还原恢复模式」切换。`,
    okLabel: mode === 'link_only' ? '修复链接' : `开始${verb}`,
  };
}
