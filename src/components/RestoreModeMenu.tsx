// 顶部栏还原模式菜单
// 只负责选择与展示当前模式，实际行为由后端按同一份设置执行

import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Archive, Check, ChevronDown, FolderInput, Link2 } from 'lucide-react';
import { RESTORE_MODES, restoreModeMeta, type RestoreMode } from '../utils/restoreMode';

interface RestoreModeMenuProps {
  value: RestoreMode;
  onChange: (mode: RestoreMode) => void;
}

/** 每个模式一个语义色：仅链接=主色（不动数据）、保留=警示（双份占用）、移动=成功（干净搬回） */
const MODE_ICON: Record<RestoreMode, typeof Link2> = {
  link_only: Link2,
  keep_data: Archive,
  move: FolderInput,
};

const MODE_COLOR: Record<RestoreMode, string> = {
  link_only: 'var(--color-primary)',
  keep_data: 'var(--color-warning)',
  move: 'var(--color-success)',
};

export default function RestoreModeMenu({ value, onChange }: RestoreModeMenuProps) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [menuPosition, setMenuPosition] = useState<{ top: number; right: number } | null>(null);

  const current = restoreModeMeta(value);
  const CurrentIcon = MODE_ICON[value];

  useEffect(() => {
    if (!open) return;

    function updatePosition() {
      if (!triggerRef.current) return;
      const rect = triggerRef.current.getBoundingClientRect();
      setMenuPosition({
        top: rect.bottom + 6,
        right: Math.max(8, window.innerWidth - rect.right),
      });
    }

    updatePosition();
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open]);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      const insideTrigger = triggerRef.current?.contains(target) ?? false;
      const insideMenu = menuRef.current?.contains(target) ?? false;
      if (!insideTrigger && !insideMenu) setOpen(false);
    }

    function handleEscape(event: KeyboardEvent) {
      if (event.key === 'Escape') setOpen(false);
    }

    window.addEventListener('mousedown', handleClickOutside);
    window.addEventListener('keydown', handleEscape);
    return () => {
      window.removeEventListener('mousedown', handleClickOutside);
      window.removeEventListener('keydown', handleEscape);
    };
  }, []);

  return (
    <div className="relative flex-shrink-0">
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        title={`还原恢复模式：${current.label}（${current.summary}）`}
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex items-center gap-1.5 h-7 px-2 rounded-md border transition-colors hover:bg-[var(--bg-hover)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]/30"
        style={{ borderColor: 'var(--border-color)', background: 'var(--bg-input)' }}
      >
        <CurrentIcon className="w-3.5 h-3.5 flex-shrink-0" style={{ color: MODE_COLOR[value] }} />
        {/* 窄窗口只留图标，避免标题栏被挤爆 */}
        <span
          className="text-[11px] whitespace-nowrap max-[880px]:hidden"
          style={{ color: 'var(--text-secondary)' }}
        >
          还原恢复模式：{current.short}
        </span>
        <ChevronDown
          className={`w-3 h-3 flex-shrink-0 transition-transform ${open ? 'rotate-180' : ''}`}
          style={{ color: 'var(--text-tertiary)' }}
        />
      </button>

      {open && menuPosition && createPortal(
        <div
          ref={menuRef}
          role="menu"
          className="glass-popover fixed overflow-hidden rounded-lg bg-[var(--bg-modal)] ring-1 ring-[var(--border-color)]"
          style={{
            top: menuPosition.top,
            right: menuPosition.right,
            width: '292px',
            zIndex: 2000,
            boxShadow: 'var(--shadow-lg)',
          }}
        >
          <div className="px-3 pt-2 pb-1">
            <span className="text-[10px] uppercase tracking-wide" style={{ color: 'var(--text-tertiary)' }}>
              还原恢复模式
            </span>
          </div>

          {RESTORE_MODES.map((mode) => {
            const Icon = MODE_ICON[mode.key];
            const selected = mode.key === value;
            return (
              <button
                key={mode.key}
                type="button"
                role="menuitemradio"
                aria-checked={selected}
                onClick={() => {
                  onChange(mode.key);
                  setOpen(false);
                }}
                className="w-full px-3 py-2 text-left flex items-start gap-2 transition-colors hover:bg-[var(--bg-hover)]"
                style={{ background: selected ? 'var(--bg-row-selected)' : 'transparent' }}
              >
                <Icon
                  className="w-4 h-4 mt-0.5 flex-shrink-0"
                  style={{ color: MODE_COLOR[mode.key] }}
                />
                <span className="min-w-0 flex-1">
                  <span
                    className="block text-[12px] font-medium"
                    style={{ color: selected ? MODE_COLOR[mode.key] : 'var(--text-primary)' }}
                  >
                    {mode.label}
                  </span>
                  <span className="block text-[11px] leading-snug" style={{ color: 'var(--text-tertiary)' }}>
                    {mode.summary}
                  </span>
                </span>
                {selected && <Check className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" style={{ color: MODE_COLOR[mode.key] }} />}
              </button>
            );
          })}

          <div className="px-3 py-2 border-t" style={{ borderColor: 'var(--border-color)' }}>
            <span className="text-[10px] leading-snug" style={{ color: 'var(--text-tertiary)' }}>
              还原恢复时按此模式执行，弹窗会提示当前模式
            </span>
          </div>
        </div>
      , document.body)}
    </div>
  );
}
