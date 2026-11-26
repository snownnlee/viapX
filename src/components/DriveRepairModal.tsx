// 迁移记录盘符修正弹窗
//
// 记录里写死绝对路径，数据盘盘符一变就全部失效。这里选定"旧盘符 → 新盘符"，
// 先预览会被改写的记录与链接，确认后一次性改写记录、兜底元数据、自定义文件夹，
// 并把原路径上的目录联接重建到新位置。

import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertTriangle, ArrowRight, CheckCircle2, Link2, Loader2, RefreshCw } from 'lucide-react';
import Modal from './Modal';
import FilterSelect from './FilterSelect';
import {
  DriveRepairDrives,
  DriveRepairPreview,
  DriveRepairResult,
} from '../types';

interface DriveRepairModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** 修正成功后回调，用于刷新记录列表 */
  onApplied: (result: DriveRepairResult) => void;
}

export default function DriveRepairModal({ isOpen, onClose, onApplied }: DriveRepairModalProps) {
  const [drives, setDrives] = useState<DriveRepairDrives>({ record_drives: [], existing_drives: [] });
  const [oldDrive, setOldDrive] = useState('F');
  const [newDrive, setNewDrive] = useState('');
  const [preview, setPreview] = useState<DriveRepairPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState('');

  // 打开时拉取盘符列表并给出一组默认值
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setPreview(null);
    setError('');
    (async () => {
      try {
        const data = await invoke<DriveRepairDrives>('list_drive_repair_drives');
        if (cancelled) return;
        setDrives(data);
        // 记录里有、但系统上已不存在的盘符最可能是"改过盘符"的那个
        const missing = data.record_drives.find(drive => !data.existing_drives.includes(drive));
        const oldDefault = missing ?? data.record_drives[0] ?? 'F';
        const newDefault = data.existing_drives.find(drive => drive !== oldDefault) ?? '';
        setOldDrive(oldDefault);
        setNewDrive(newDefault);
      } catch (err) {
        if (!cancelled) setError(`读取盘符失败：${err}`);
      }
    })();
    return () => { cancelled = true; };
  }, [isOpen]);

  const oldOptions = useMemo(
    () => drives.record_drives.map(drive => ({ value: drive, label: `${drive}:` })),
    [drives.record_drives],
  );
  const newOptions = useMemo(
    () => drives.existing_drives
      .filter(drive => drive !== oldDrive)
      .map(drive => ({ value: drive, label: `${drive}:` })),
    [drives.existing_drives, oldDrive],
  );

  async function handlePreview() {
    if (!oldDrive || !newDrive) {
      setError('请先选择旧盘符与新盘符');
      return;
    }
    setLoading(true);
    setError('');
    setPreview(null);
    try {
      const data = await invoke<DriveRepairPreview>('preview_drive_repair', {
        oldDrive,
        newDrive,
      });
      setPreview(data);
    } catch (err) {
      setError(`${err}`);
    } finally {
      setLoading(false);
    }
  }

  async function handleApply() {
    if (!preview) return;
    setApplying(true);
    setError('');
    try {
      const result = await invoke<DriveRepairResult>('apply_drive_repair', {
        oldDrive,
        newDrive,
      });
      onApplied(result);
      onClose();
    } catch (err) {
      setError(`修正失败：${err}`);
    } finally {
      setApplying(false);
    }
  }

  const totalChanges = preview
    ? preview.target_items.length + preview.original_items.length + preview.metadata_count
      + preview.custom_folder_count + preview.links.length
    : 0;
  const missingNewPaths = preview
    ? preview.target_items.filter(item => !item.new_exists).length
      + preview.original_items.filter(item => !item.new_exists).length
    : 0;
  const sampleItems = preview ? [...preview.target_items, ...preview.original_items].slice(0, 6) : [];

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title="盘符修正"
      width={640}
      footer={(
        <div className="flex items-center justify-end gap-2">
          <button onClick={onClose} className="btn h-8 text-[12px]">取消</button>
          <button
            onClick={handleApply}
            disabled={!preview || applying || totalChanges === 0}
            className="btn btn-primary h-8 text-[12px]"
          >
            {applying ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : null}
            {applying ? '正在修正' : '应用修正'}
          </button>
        </div>
      )}
    >
      <div className="space-y-4">
        <p className="text-[12px]" style={{ color: 'var(--text-secondary)' }}>
          数据盘盘符变化后（换盘、重装系统后盘序改变），迁移记录里的目标路径会全部失效。
          选定旧盘符与新盘符后先预览，确认无误再应用；记录、绿色软件元数据、自定义文件夹会一起修正，
          原路径上的目录联接会重建到新位置。
        </p>

        <div className="flex items-end gap-3">
          <div className="flex-1 min-w-0">
            <div className="text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>旧盘符（记录里写的）</div>
            <FilterSelect
              value={oldDrive}
              options={oldOptions}
              onChange={(value) => { setOldDrive(value); setPreview(null); }}
              className="w-full"
            />
          </div>
          <ArrowRight className="w-4 h-4 mb-2 flex-shrink-0" style={{ color: 'var(--text-tertiary)' }} />
          <div className="flex-1 min-w-0">
            <div className="text-[11px] mb-1" style={{ color: 'var(--text-tertiary)' }}>新盘符（数据现在所在）</div>
            <FilterSelect
              value={newDrive}
              options={newOptions}
              onChange={(value) => { setNewDrive(value); setPreview(null); }}
              className="w-full"
            />
          </div>
          <button onClick={handlePreview} disabled={loading || !newDrive} className="btn h-8 text-[12px] flex-shrink-0">
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? 'animate-spin' : ''}`} />
            预览
          </button>
        </div>

        {error && (
          <div className="text-[12px] flex items-start gap-1.5" style={{ color: 'var(--color-danger)' }}>
            <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
            <span className="break-all">{error}</span>
          </div>
        )}

        {preview && (
          <div className="rounded-lg border p-3 space-y-3" style={{ borderColor: 'var(--border-color)', background: 'var(--bg-inset)' }}>
            <div className="grid grid-cols-3 gap-2 text-[11px]">
              <Stat label="目标路径" value={preview.target_items.length} />
              <Stat label="原路径" value={preview.original_items.length} />
              <Stat label="兜底元数据" value={preview.metadata_count} />
              <Stat label="自定义文件夹" value={preview.custom_folder_count} />
              <Stat label="重建链接" value={preview.links.length} />
              <div className="rounded border px-2 py-1.5" style={{ borderColor: 'var(--border-color)' }}>
                <div style={{ color: 'var(--text-tertiary)' }}>新盘状态</div>
                <div className="font-medium" style={{ color: preview.new_drive_exists ? 'var(--color-success)' : 'var(--color-warning)' }}>
                  {preview.new_drive_exists ? '已就绪' : '未检测到'}
                </div>
              </div>
            </div>

            {!preview.new_drive_exists && (
              <div className="text-[11px] flex items-start gap-1.5" style={{ color: 'var(--color-warning)' }}>
                <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
                <span>{preview.new_drive}: 当前不存在。记录仍可修正，但目录链接不会重建，插上磁盘后再点一次即可。</span>
              </div>
            )}
            {missingNewPaths > 0 && (
              <div className="text-[11px] flex items-start gap-1.5" style={{ color: 'var(--color-warning)' }}>
                <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
                <span>{missingNewPaths} 条路径在新位置上找不到（可能是已删除或已还原的记录），列表里标注为「不存在」。</span>
              </div>
            )}

            {preview.links.length > 0 && (
              <div className="text-[11px] flex items-start gap-1.5" style={{ color: 'var(--text-secondary)' }}>
                <Link2 className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" style={{ color: 'var(--color-primary)' }} />
                <span>将重建 {preview.links.length} 个原路径链接（新目标不存在时保持原样，只提示不删除）。</span>
              </div>
            )}

            {sampleItems.length > 0 && (
              <div className="space-y-1">
                <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>改写示例</div>
                <div className="max-h-40 overflow-y-auto space-y-1">
                  {sampleItems.map((item) => (
                    <div key={`${item.id}-${item.from}`} className="text-[11px] flex items-start gap-1.5">
                      <span className="flex-shrink-0" style={{ color: item.new_exists ? 'var(--color-success)' : 'var(--color-warning)' }}>
                        {item.new_exists ? <CheckCircle2 className="w-3 h-3 mt-0.5" /> : <AlertTriangle className="w-3 h-3 mt-0.5" />}
                      </span>
                      <span className="min-w-0 break-all" style={{ color: 'var(--text-secondary)' }}>
                        {item.from} → {item.to}
                        {!item.new_exists && <span style={{ color: 'var(--color-warning)' }}>（不存在）</span>}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {totalChanges === 0 && (
              <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                没有需要修正的内容：记录里没有以 {preview.old_drive}: 开头的路径。
              </div>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded border px-2 py-1.5" style={{ borderColor: 'var(--border-color)' }}>
      <div style={{ color: 'var(--text-tertiary)' }}>{label}</div>
      <div className="font-medium" style={{ color: 'var(--text-primary)' }}>{value}</div>
    </div>
  );
}
