import { test, suite, assert, assertEq, assertThrows, withFixture } from './harness.js';
import {
 clamp, hasValue, snapshotForm, isFormDirty, addFieldError, clampNumberOnBlur,
} from '../js/utils/form.js';

suite('form', () => {
 // clamp
 test('clamp(5, 0, 10) === 5 (in range)', () => assertEq(clamp(5, 0, 10), 5));
 test('clamp(-1, 0, 10) === 0 (clamps low)', () => assertEq(clamp(-1, 0, 10), 0));
 test('clamp(15, 0, 10) === 10 (clamps high)', () => assertEq(clamp(15, 0, 10), 10));

 // hasValue
 test('hasValue(0) === true (0 is a value)', () => assertEq(hasValue(0), true));
 test('hasValue("") === true (empty string is a value)', () => assertEq(hasValue(''), true));
 test('hasValue(null) === false', () => assertEq(hasValue(null), false));
 test('hasValue(undefined) === false', () => assertEq(hasValue(undefined), false));

 // snapshotForm / isFormDirty
 const formHtml = `
 <input id="name" value="alpha">
 <input id="chk" type="checkbox" checked>
 <select id="sel"><option value="x" selected>x</option></select>
 `;
 test('snapshotForm: same state → same string', () =>
 withFixture(formHtml, root => {
 const a = snapshotForm(root), b = snapshotForm(root);
 assertEq(a, b);
 assert(a.includes('name=alpha'), 'snapshot includes name=alpha');
 assert(a.includes('chk=1'), 'snapshot marks a checked checkbox as 1');
 }));
 test('snapshotForm: changed checkbox → different string', () =>
 withFixture(formHtml, root => {
 const before = snapshotForm(root);
 root.querySelector('#chk').checked = false;
 assert(before !== snapshotForm(root), 'unchecking must change the snapshot');
 }));
 test('snapshotForm: changed input → different string', () =>
 withFixture(formHtml, root => {
 const before = snapshotForm(root);
 root.querySelector('#name').value = 'beta';
 assert(before !== snapshotForm(root), 'typing must change the snapshot');
 }));
 test('isFormDirty: empty snapshot → false', () =>
 withFixture(formHtml, root => assertEq(isFormDirty(root, ''), false)));
 test('isFormDirty: same snapshot → false', () =>
 withFixture(formHtml, root => assertEq(isFormDirty(root, snapshotForm(root)), false)));
 test('isFormDirty: different state → true', () =>
 withFixture(formHtml, root => {
 const snap = snapshotForm(root);
 root.querySelector('#name').value = 'changed';
 assertEq(isFormDirty(root, snap), true);
 }));

 // addFieldError
 test('addFieldError adds .error class and a .field-error message before the hint', () =>
 withFixture('<div id="field"><span class="hint">h</span></div>', root => {
 const field = root.querySelector('#field');
 addFieldError(field, 'Required');
 assert(field.classList.contains('error'), 'field gets .error class');
 const err = field.querySelector('.field-error');
 assert(err, '.field-error div inserted');
 assertEq(err.textContent, 'Required');
 assert(field.querySelector('.field-error + .hint'), 'error div must precede the hint');
 }));
 test('addFieldError appends the error div when no hint is present', () =>
 withFixture('<div id="field"></div>', root => {
 const field = root.querySelector('#field');
 addFieldError(field, 'Bad');
 assert(field.querySelector('.field-error'), '.field-error appended');
 }));
 test('addFieldError throws on a null field (fail fast, no silent skip)', () =>
 assertThrows(() => addFieldError(null, 'x'), 'addFieldError(null) must throw'));

 // clampNumberOnBlur
 test('clampNumberOnBlur clamps an out-of-range number input', () =>
 withFixture('<input id="n" type="number" min="0" max="10" value="42">', root => {
 const input = root.querySelector('#n');
 clampNumberOnBlur({ target: input });
 assertEq(Number(input.value), 10);
 }));
 test('clampNumberOnBlur ignores non-number inputs', () =>
 withFixture('<input id="t" type="text" min="0" max="10" value="hello">', root => {
 const input = root.querySelector('#t');
 clampNumberOnBlur({ target: input });
 assertEq(input.value, 'hello');
 }));
});
