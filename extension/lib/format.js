'use strict';

/**
 * Number formatting for the extension's two pages.
 *
 * Deliberately a byte-for-byte match of `format_bytes` / `format_speed` /
 * `format_duration` in crates/fdm-core/src/progress.rs. The popup and the
 * desktop app show the same download at the same moment; if one said "1.2 GB"
 * and the other "1.15 GiB" the user would reasonably assume one of them was
 * lying.
 */

const FDM_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];

function fdmFormatBytes(bytes) {
  const n = Number(bytes);
  if (!Number.isFinite(n) || n < 0) return '—';

  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < FDM_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // Whole bytes get no decimal point, exactly as the engine prints them.
  return unit === 0 ? `${Math.round(n)} B` : `${value.toFixed(2)} ${FDM_UNITS[unit]}`;
}

function fdmFormatSpeed(bytesPerSecond) {
  const n = Number(bytesPerSecond);
  if (!Number.isFinite(n) || n <= 0) return '—';
  return `${fdmFormatBytes(Math.trunc(n))}/s`;
}

function fdmFormatDuration(seconds) {
  const total = Number(seconds);
  if (!Number.isFinite(total) || total < 0) return '—';

  const secs = Math.trunc(total);
  const h = Math.trunc(secs / 3600);
  const m = Math.trunc((secs % 3600) / 60);
  const s = secs % 60;

  const pad = (v) => String(v).padStart(2, '0');
  if (h > 0) return `${h}h ${pad(m)}m ${pad(s)}s`;
  if (m > 0) return `${m}m ${pad(s)}s`;
  return `${s}s`;
}

/** 0–100, clamped. `null` totals give 0 rather than NaN. */
function fdmPercent(downloaded, total) {
  const d = Number(downloaded);
  const t = Number(total);
  if (!Number.isFinite(d) || !Number.isFinite(t) || t <= 0) return 0;
  return Math.min(100, Math.max(0, (d / t) * 100));
}
