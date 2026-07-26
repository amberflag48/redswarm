// Format utilities - mirrors data::units on the Rust side.

export const BYTE_UNITS = [
  { v: 1073741824, label: 'GiB/s', single: 'GiB' },
  { v: 1048576,    label: 'MiB/s', single: 'MiB' },
  { v: 1024,       label: 'KiB/s', single: 'KiB' },
  { v: 1,          label: 'B/s',   single: 'B'   },
];

export function byteUnitOptions(selected) {
  return BYTE_UNITS.slice().reverse().map(u => {
    const sel = String(u.v) === String(selected) ? ' selected' : '';
    return `<option value="${u.v}"${sel}>${u.label}</option>`;
  }).join('');
}

// Amount-unit options (no /s) - for byte-total fields like goal targets.
// Mirrors the Rust render_byte_amount_options.
export function byteAmountOptions(selected) {
  return BYTE_UNITS.slice().reverse().map(u => {
    const sel = String(u.v) === String(selected) ? ' selected' : '';
    return `<option value="${u.v}"${sel}>${u.single}</option>`;
  }).join('');
}

// Mirrors data::units::fmt_bytes: integer bytes (no decimals) below 1024,
// 2-decimal precision for KiB/MiB/GiB. The bytes unit (v === 1) renders the
// raw integer, so 1023 → "1023 B" not "1023.00 B" - byte-identical with Rust.
export function formatBytes(n) {
  for (let i = 0; i < BYTE_UNITS.length; i++) {
    const u = BYTE_UNITS[i];
    if (n >= u.v) {
      const val = u.v === 1 ? n : (n / u.v).toFixed(2);
      return val + ' ' + u.single;
    }
  }
  return n + ' B';
}

// Mirrors data::units::fmt_speed_bps.
export function formatSpeedBps(bps) { return formatBytes(bps) + '/s'; }

// Mirrors data::units::fmt_speed_cell.
export function formatSpeedCell(upBps, downBps, showDownload) {
  const up = formatBytes(upBps) + '/s ↑';
  if (!showDownload) return up;
  return up + ' ' + formatBytes(downBps) + '/s ↓';
}

// Mirrors data::units::fmt_duration.
export function formatDuration(s) {
  if (s < 60) return s + 's';
  if (s < 3600) return Math.floor(s / 60) + 'm ' + (s % 60) + 's';
  if (s < 86400) return Math.floor(s / 3600) + 'h ' + Math.floor((s % 3600) / 60) + 'm';
  return Math.floor(s / 86400) + 'd ' + Math.floor((s % 86400) / 3600) + 'h';
}

export function setSpeedField(valId, unitId, bps) {
  for (let i = 0; i < BYTE_UNITS.length; i++) {
    if (bps >= BYTE_UNITS[i].v) {
      document.getElementById(valId).value = parseFloat((bps / BYTE_UNITS[i].v).toFixed(2));
      document.getElementById(unitId).value = String(BYTE_UNITS[i].v);
      return;
    }
  }
  document.getElementById(valId).value = 0;
  document.getElementById(unitId).value = String(BYTE_UNITS[1].v); // MiB default
}

// Set a byte-amount field (value + unit) from a raw byte count. Like
// setSpeedField but defaults to MiB (not B) when the value is 0, so an empty
// goal-target field shows "0 MiB" instead of "0 B". Used by goal-target
// fields (total amounts, not speeds).
export function setByteField(valId, unitId, bytes) {
  for (let i = 0; i < BYTE_UNITS.length; i++) {
    if (bytes >= BYTE_UNITS[i].v) {
      document.getElementById(valId).value = parseFloat((bytes / BYTE_UNITS[i].v).toFixed(2));
      document.getElementById(unitId).value = String(BYTE_UNITS[i].v);
      return;
    }
  }
  document.getElementById(valId).value = 0;
  document.getElementById(unitId).value = String(BYTE_UNITS[1].v); // MiB
}

export function getSpeedBps(valId, unitId) {
  const v = parseFloat(document.getElementById(valId).value);
  const u = parseInt(document.getElementById(unitId).value, 10);
  return Math.round(v * u);
}

// ETA in seconds for a goal given remaining bytes + current speed. Returns:
//   0    when the goal is reached (remaining <= 0)
//   null when the speed is unknown (bps <= 0) - caller shows a placeholder
//   >0   the ceil(remaining / bps) ETA in seconds
// Shared by the topbar aggregate + the log-panel per-task tiles.
export function goalEtaSeconds(remaining, bps) {
  if (remaining <= 0) return 0;
  if (bps <= 0) return null;
  return Math.ceil(remaining / bps);
}
// E2E cache-bust test marker
