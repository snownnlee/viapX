// 应用批量操作能力判定
// 批量模式由第一个被勾选的应用决定：可迁移 → 批量迁移；可还原 → 批量还原。

import { findMigratedParent, type MigratedParent } from './migratedParent';

export type BatchMode = 'migrate' | 'restore';

export interface AppBatchCaps {
  /** 可以加入批量迁移 */
  canMigrate: boolean;
  /** 可以加入批量还原 */
  canRestore: boolean;
}

interface BatchCapsInput {
  installLocation: string;
  /** 该路径本身是否有活跃迁移记录（即当前处于迁移状态） */
  isMigrated: boolean;
  /** 保留模式还原后仍保留的副本路径 */
  keptRestoreTarget?: string;
  migratedParents: MigratedParent[];
  isViap: boolean;
}

/**
 * 判定单个应用在批量操作里的可用能力
 *
 * - Viap 自身与"随父目录迁移"的应用既不能批量迁移也不能批量还原
 * - 未迁移的可以迁移；已迁移或有保留副本的可以还原（有副本时两者皆可）
 */
export function appBatchCaps(input: BatchCapsInput): AppBatchCaps {
  const { installLocation, isMigrated, keptRestoreTarget, migratedParents, isViap } = input;
  if (isViap || findMigratedParent(installLocation, migratedParents)) {
    return { canMigrate: false, canRestore: false };
  }
  return {
    canMigrate: !isMigrated,
    canRestore: isMigrated || !!keptRestoreTarget,
  };
}

/** 应用在勾选集合里的稳定 key（与列表渲染保持一致） */
export function appBatchKey(app: { registry_path?: string; install_location: string }): string {
  return app.registry_path || app.install_location;
}
