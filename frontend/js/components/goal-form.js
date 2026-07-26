// Shared goal-form logic - the single source of truth for populate, collect,
// and conditional visibility of goal fields across the task modal, goals
// modal, and settings modal. Each modal passes an `ids` object mapping
// logical names to actual DOM element IDs, so the shared logic is agnostic
// to each modal's naming convention (hyphens for task/goals modals, underscores
// for settings). Adding a new goal field = edit this module + one HTML field
// in each modal template - not 3 duplicate logic implementations.
//
// The `ids` object:
//   enable, direction, uploadTargetVal, uploadTargetUnit, downloadTargetVal,
//   downloadTargetUnit, timeVal, timeUnit (or targetSecs for raw-seconds mode),
//   action, reachedVal, reachedUnit, [collapsibleBlock], [swapHint]

import { setSpeedField, setByteField, getSpeedBps } from '../utils/format.js';

/// Populate goal fields from a goal config object. `ids` maps logical names
/// to actual DOM element IDs. `opts.timeAsPicker` selects the value+unit
/// picker (true) vs raw-seconds input (false) for the time field.
export function populateGoalFields(ids, g, opts = {}) {
  const el = id => document.getElementById(id);
  if (el(ids.enable)) el(ids.enable).checked = !!g.enabled;
  if (el(ids.direction)) el(ids.direction).value = g.direction || 'upload';
  setByteField(ids.uploadTargetVal, ids.uploadTargetUnit, g.upload_target ?? 0);
  setByteField(ids.downloadTargetVal, ids.downloadTargetUnit, g.download_target ?? 0);
  if (opts.timeAsPicker) {
    setGoalTimeField(ids.timeVal, ids.timeUnit, g.target_secs ?? 0);
  } else {
    if (el(ids.targetSecs)) el(ids.targetSecs).value = g.target_secs ?? 0;
  }
  if (el(ids.action)) el(ids.action).value = g.reached_action || 'stop';
  setSpeedField(ids.reachedVal, ids.reachedUnit, g.reached_bps ?? 0);
}

/// Collect goal fields into a config object. The inverse of
/// populateGoalFields. Returns { enabled, direction, upload_target,
/// download_target, target_secs, reached_action, reached_bps }.
export function collectGoalFields(ids, opts = {}) {
  const el = id => document.getElementById(id);
  return {
    enabled: el(ids.enable) ? el(ids.enable).checked : false,
    direction: el(ids.direction) ? el(ids.direction).value : 'upload',
    upload_target: getSpeedBps(ids.uploadTargetVal, ids.uploadTargetUnit),
    download_target: getSpeedBps(ids.downloadTargetVal, ids.downloadTargetUnit),
    target_secs: opts.timeAsPicker
      ? getSpeedBps(ids.timeVal, ids.timeUnit)
      : (parseInt(el(ids.targetSecs)?.value) || 0),
    reached_action: el(ids.action) ? el(ids.action).value : 'stop',
    reached_bps: getSpeedBps(ids.reachedVal, ids.reachedUnit),
  };
}

/// Update conditional visibility of goal sub-fields. Shared across all three
/// modals - the one place the rule lives. `ids` maps logical names to actual
/// DOM element IDs. `opts`:
///   - `collapsibleBlock`: if given, toggle `.open` on this element when
///     the goal is enabled (task modal uses a .conditional wrapper).
///   - `hideAllWhenDisabled`: if true, add `.hidden` to each sub-field's
///     `.field` parent when the goal is disabled (settings modal style).
///   - `swapHint`: if given, swap its textContent by direction (task modal
///     swaps the upload-target hint between "upload bytes" / "download
///     bytes").
///   - `timeAsPicker`: affects which fields to include in hideAllWhenDisabled.
export function updateGoalFieldVisibility(ids, opts = {}) {
  const el = id => document.getElementById(id);
  const enable = el(ids.enable);
  if (!enable) return;
  const enabled = enable.checked;
  const direction = el(ids.direction);
  const action = el(ids.action);
  const dir = direction ? direction.value : 'upload';

  // Collapse / expand the whole block (task modal's .conditional pattern).
  if (ids.collapsibleBlock) {
    const block = el(ids.collapsibleBlock);
    if (block) block.classList.toggle('open', enabled);
  }

  // List of sub-field element IDs to toggle when disabled.
  const subFieldIds = [ids.direction, ids.uploadTargetVal, ids.downloadTargetVal];
  if (opts.timeAsPicker) subFieldIds.push(ids.timeVal);
  else if (ids.targetSecs) subFieldIds.push(ids.targetSecs);
  subFieldIds.push(ids.action, ids.reachedVal);

  if (opts.hideAllWhenDisabled) {
    for (const fid of subFieldIds) {
      const e = el(fid);
      if (e) {
        const field = e.closest('.field');
        if (field) field.classList.toggle('hidden', !enabled);
      }
    }
  }

  if (!enabled && !opts.hideAllWhenDisabled && !ids.collapsibleBlock) return;

  // Download-target field: visible only in download_and_upload.
  const dlEl = el(ids.downloadTargetVal);
  if (dlEl) {
    const dlField = dlEl.closest('.field');
    if (dlField) dlField.classList.toggle('hidden', dir !== 'download_and_upload');
  }

  // Swap the upload-target hint text by direction (task modal only).
  // In download_and_upload mode, the hint clarifies it's the upload portion.
  if (ids.swapHint) {
    const hint = el(ids.swapHint);
    if (hint) hint.textContent = 'Cumulative upload bytes to reach.';
  }

  // Reached-speed field: visible only when action is continue_custom.
  const bpsEl = el(ids.reachedVal);
  if (bpsEl) {
    const bpsField = bpsEl.closest('.field');
    const custom = enabled && action && action.value === 'continue_custom';
    if (bpsField) bpsField.classList.toggle('hidden', !custom);
  }
}

/// Set the goal-time (value + unit) inputs from a seconds value. Picks the
/// largest of h/m/s that fits so the display stays compact (3600 → 1h, 1800
/// → 30m, 90 → 90s). The unit <option> values are the multipliers 3600/60/1.
function setGoalTimeField(valId, unitId, secs) {
  const units = [3600, 60, 1];
  for (const u of units) {
    if (secs >= u) {
      document.getElementById(valId).value = parseFloat((secs / u).toFixed(2));
      document.getElementById(unitId).value = String(u);
      return;
    }
  }
  document.getElementById(valId).value = 0;
  document.getElementById(unitId).value = '1';
}
