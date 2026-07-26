import { test, suite, assert, assertEq, withFixture } from './harness.js';
import { escHtml, escAttr, focusFirst, setSegmented } from '../js/utils/dom-helpers.js';

// Security-critical escaping + focus/segmented helpers. escHtml/escAttr guard
// every dynamic string that lands in innerHTML or an attribute, so regressions
// here are XSS vectors - asserted with exact strings where deterministic and
// not-contains/contains for the tag-injection cases (browser serialization of
// `>` is irrelevant once `<` is escaped, so the assertions stay robust).

suite('escHtml', () => {
  test("escHtml('<script>alert(1)</script>') cannot re-enter a live <script> tag", () => {
    const out = escHtml('<script>alert(1)</script>');
    assert(!out.includes('<script>'), 'must not contain a live <script> tag');
    assert(out.includes('&lt;script&gt;'), 'must escape both < and >');
  });
  test("escHtml('&') === '&amp;'", () => assertEq(escHtml('&'), '&amp;'));
  test('escHtml(null) === ""', () => assertEq(escHtml(null), ''));
  test('escHtml(undefined) === ""', () => assertEq(escHtml(undefined), ''));
  test('escHtml(0) === "0" (0 is a value, not coerced to empty)', () => assertEq(escHtml(0), '0'));
  test("escHtml('hello world') === 'hello world' (safe text unchanged)", () =>
    assertEq(escHtml('hello world'), 'hello world'));
  test("escHtml('<img src=x onerror=alert(1)>') cannot re-enter a live <img tag", () => {
    const out = escHtml('<img src=x onerror=alert(1)>');
    assert(!out.includes('<img'), 'must not contain a live <img tag');
    assert(out.includes('&lt;img'), 'must escape <');
  });
});

suite('escAttr', () => {
  test('escAttr escapes double quotes (breaks attribute-boundary injection)', () =>
    assertEq(escAttr('hello"world'), 'hello&quot;world'));
  test('escAttr escapes <', () => assertEq(escAttr('a<b'), 'a&lt;b'));
  test('escAttr escapes & exactly once (no double-escaping)', () =>
    assertEq(escAttr('a&b'), 'a&amp;b'));
  test('escAttr(null) === ""', () => assertEq(escAttr(null), ''));
  test('escAttr(undefined) === ""', () => assertEq(escAttr(undefined), ''));
  test('escAttr escapes all four special chars together', () =>
    assertEq(escAttr('<a href="x">&'), '&lt;a href=&quot;x&quot;&gt;&amp;'));
});

suite('focusFirst', () => {
  test('focusFirst focuses the first button over later inputs/selects', () =>
    withFixture('<div><button>No</button><input><select></div>', root => {
      focusFirst(root);
      assert(document.activeElement === root.querySelector('button'),
        'the first button should hold focus, not the input/select');
    }));
  test('focusFirst skips .modal-close buttons and focuses the next focusable', () =>
    withFixture('<div><button class="modal-close">x</button><input></div>', root => {
      focusFirst(root);
      assert(document.activeElement === root.querySelector('input'),
        'modal-close button is skipped, input gets focus');
    }));
});

suite('setSegmented', () => {
  const segmentedHtml = `
    <div class="segmented" aria-label="test">
      <button data-value="a">A</button>
      <button data-value="b">B</button>
      <button data-value="c">C</button>
    </div>`;
  test("setSegmented('test','b') activates only the b button", () =>
    withFixture(segmentedHtml, root => {
      setSegmented('test', 'b');
      const a = root.querySelector('[data-value="a"]');
      const b = root.querySelector('[data-value="b"]');
      const c = root.querySelector('[data-value="c"]');
      assert(b.classList.contains('active'), 'b gets .active');
      assertEq(b.getAttribute('aria-checked'), 'true');
      assertEq(b.tabIndex, 0);
      assert(!a.classList.contains('active'), 'a not active');
      assertEq(a.getAttribute('aria-checked'), 'false');
      assertEq(a.tabIndex, -1);
      assert(!c.classList.contains('active'), 'c not active');
      assertEq(c.getAttribute('aria-checked'), 'false');
      assertEq(c.tabIndex, -1);
    }));
  test("setSegmented('test','a') moves activation to a and clears b", () =>
    withFixture(segmentedHtml, root => {
      setSegmented('test', 'b');
      setSegmented('test', 'a');
      const a = root.querySelector('[data-value="a"]');
      const b = root.querySelector('[data-value="b"]');
      assert(a.classList.contains('active'), 'a now active');
      assertEq(a.getAttribute('aria-checked'), 'true');
      assert(!b.classList.contains('active'), 'b deactivated');
      assertEq(b.getAttribute('aria-checked'), 'false');
    }));
});
