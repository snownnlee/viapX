// 还原模式上下文
// 由 App 注入当前模式，供各还原入口调整按钮文案与确认弹窗；实际行为由后端按同一份设置执行。

import { createContext, useContext } from 'react';
import { DEFAULT_RESTORE_MODE, type RestoreMode } from '../utils/restoreMode';

export const RestoreModeContext = createContext<RestoreMode>(DEFAULT_RESTORE_MODE);

export function useRestoreMode(): RestoreMode {
  return useContext(RestoreModeContext);
}
