import { test, suite, assert, assertEq, withFixture } from './harness.js';
import {
  BYTE_UNITS, byteUnitOptions, byteAmountOptions, formatBytes, formatSpeedBps,
  formatSpeedCell, formatDuration, setSpeedField, setByteField, getSpeedBps, goalEtaSeconds,
} from '../js/utils/format.js';

// formatBytes / formatSpeedBps / formatDuration must stay byte-identical with
// the Rust data::units formatters (see src/data/units.rs). These tests pin the
// JS to the canonical Rust output so a table cell looks the same whether it
// arrives via the initial snapshot or a live SSE update.
suite('format', () => {
 // formatBytes
 test('formatBytes(0) → "0 B"', () => assertEq(formatBytes(0), '0 B'));
 test('formatBytes(1023) → "1023 B" (integer bytes, no decimals)', () =>
 assertEq(formatBytes(1023), '1023 B'));
 test('formatBytes(1024) → "1.00 KiB"', () => assertEq(formatBytes(1024), '1.00 KiB'));
 test('formatBytes(1536) → "1.50 KiB"', () => assertEq(formatBytes(1536), '1.50 KiB'));
 test('formatBytes(1048576) → "1.00 MiB"', () => assertEq(formatBytes(1048576), '1.00 MiB'));
 test('formatBytes(1073741824) → "1.00 GiB"', () => assertEq(formatBytes(1073741824), '1.00 GiB'));
 test('formatBytes(1) → "1 B" (mirrors Rust fmt_bytes(1))', () => assertEq(formatBytes(1), '1 B'));

 // formatSpeedBps
 test('formatSpeedBps(0) → "0 B/s"', () => assertEq(formatSpeedBps(0), '0 B/s'));
 test('formatSpeedBps(1024) → "1.00 KiB/s"', () => assertEq(formatSpeedBps(1024), '1.00 KiB/s'));
 test('formatSpeedBps(1) → "1 B/s"', () => assertEq(formatSpeedBps(1), '1 B/s'));

 // formatSpeedCell
 test('formatSpeedCell hides download when showDownload=false', () =>
 assertEq(formatSpeedCell(1024, 2048, false), '1.00 KiB/s ↑'));
 test('formatSpeedCell shows both arrows when showDownload=true', () =>
 assertEq(formatSpeedCell(0, 1048576, true), '0 B/s ↑ 1.00 MiB/s ↓'));

 // formatDuration
 test('formatDuration(0) → "0s"', () => assertEq(formatDuration(0), '0s'));
 test('formatDuration(59) → "59s"', () => assertEq(formatDuration(59), '59s'));
 test('formatDuration(60) → "1m 0s"', () => assertEq(formatDuration(60), '1m 0s'));
 test('formatDuration(125) → "2m 5s"', () => assertEq(formatDuration(125), '2m 5s'));
 test('formatDuration(3600) → "1h 0m"', () => assertEq(formatDuration(3600), '1h 0m'));
 test('formatDuration(3661) → "1h 1m"', () => assertEq(formatDuration(3661), '1h 1m'));
 test('formatDuration(86399) → "23h 59m"', () => assertEq(formatDuration(86399), '23h 59m'));
 test('formatDuration(86400) → "1d 0h" (days tier)', () => assertEq(formatDuration(86400), '1d 0h'));
 test('formatDuration(90000) → "1d 1h"', () => assertEq(formatDuration(90000), '1d 1h'));
 test('formatDuration(276400) → "3d 4h"', () => assertEq(formatDuration(276400), '3d 4h'));

 // byteUnitOptions
 test('byteUnitOptions() returns 4 <option> elements', () => {
 const html = byteUnitOptions();
 assertEq((html.match(/<option/g) || []).length, 4, 'expected 4 <option> elements');
 });
 test('byteUnitOptions(1024) marks the KiB/s option as selected', () => {
 const html = byteUnitOptions(1024);
 assert(/<option[^>]*value="1024"[^>]*selected/.test(html), 'KiB/s option (value=1024) should be selected');
 assert(html.includes('KiB/s'), 'option label should be KiB/s');
 });
  test('byteUnitOptions lists units in ascending order (B, KiB, MiB, GiB)', () => {
  const vals = [...byteUnitOptions().matchAll(/value="(\d+)"/g)].map(m => Number(m[1]));
  assertEq(vals, [1, 1024, 1048576, 1073741824], 'ascending byte-unit order');
  });

  // byteAmountOptions - amount-unit labels (no /s) for goal targets.
  test('byteAmountOptions() returns 4 <option> elements', () => {
  const html = byteAmountOptions();
  assertEq((html.match(/<option/g) || []).length, 4, 'expected 4 <option> elements');
  });
  test('byteAmountOptions labels have no /s (amount, not speed)', () => {
  const html = byteAmountOptions();
  assert(!html.includes('/s'), 'amount options must not contain /s; got: ' + html);
  assert(html.includes('MiB'), 'amount options must contain MiB; got: ' + html);
  });
  test('byteAmountOptions(1048576) marks the MiB option as selected', () => {
  const html = byteAmountOptions(1048576);
  assert(/<option[^>]*value="1048576"[^>]*selected/.test(html), 'MiB option should be selected');
  });

 // BYTE_UNITS invariants
 test('BYTE_UNITS has 4 entries descending by value', () => {
 assertEq(BYTE_UNITS.length, 4);
 for (let i = 1; i < BYTE_UNITS.length; i++) {
 assert(BYTE_UNITS[i - 1].v > BYTE_UNITS[i].v, 'units must be sorted descending');
 }
 });

 // setSpeedField / getSpeedBps round-trip
 test('getSpeedBps multiplies value by unit', () =>
 withFixture('<input id="v" type="number" value="2"><input id="u" type="number" value="1048576">', () =>
 assertEq(getSpeedBps('v', 'u'), 2 * 1048576)));
 test('setSpeedField + getSpeedBps round-trip (1536 bps → 1.5 KiB → 1536 bps)', () =>
 withFixture('<input id="v" type="number"><input id="u" type="number">', () => {
 setSpeedField('v', 'u', 1536);
 assertEq(getSpeedBps('v', 'u'), 1536);
 }));
  test('setSpeedField(0) sets value=0, unit=1048576 (MiB default, not B)', () =>
  withFixture('<input id="v" type="number"><input id="u" type="number">', () => {
  setSpeedField('v', 'u', 0);
  assertEq(Number(document.getElementById('v').value), 0);
  assertEq(document.getElementById('u').value, String(1048576));
  }));

  // setByteField - amount fields default to MiB (not B) when 0.
  test('setByteField(0) sets value=0, unit=1048576 (MiB default, not B)', () =>
  withFixture('<input id="v" type="number"><input id="u" type="number">', () => {
  setByteField('v', 'u', 0);
  assertEq(Number(document.getElementById('v').value), 0);
  assertEq(document.getElementById('u').value, String(1048576));
  }));
  test('setByteField + getSpeedBps round-trip (5 MiB → 5 × 1048576)', () =>
  withFixture('<input id="v" type="number"><input id="u" type="number">', () => {
  setByteField('v', 'u', 5 * 1048576);
  assertEq(getSpeedBps('v', 'u'), 5 * 1048576);
  }));

 // goalEtaSeconds
 test('goalEtaSeconds(0, 0) → 0 (goal reached, even with no speed)', () =>
 assertEq(goalEtaSeconds(0, 0), 0));
 test('goalEtaSeconds(remaining>0, 0) → null (unknown speed)', () =>
 assertEq(goalEtaSeconds(1_000_000, 0), null));
 test('goalEtaSeconds(1_000_000, 500_000) → 2 (ceil division)', () =>
 assertEq(goalEtaSeconds(1_000_000, 500_000), 2));
 test('goalEtaSeconds(1_000_000, 300_000) → 4 (ceil, no under-shoot)', () =>
 assertEq(goalEtaSeconds(1_000_000, 300_000), 4));
 test('goalEtaSeconds(0, 500_000) → 0 (reached)', () =>
 assertEq(goalEtaSeconds(0, 500_000), 0));
});
