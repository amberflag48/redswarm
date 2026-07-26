import { test, suite, assert, assertEq } from './harness.js';
import {
 CAPTURE_STEPS, KEEPALIVE_DEFAULT,
 versionHint, compareVersions, detectKeyFormat, reconstructCaptureQuery,
} from '../js/data/capture-helpers.js';

suite('capture-helpers', () => {
 // constants
 test('CAPTURE_STEPS = ["announce","handshake","ext"]', () =>
 assertEq(CAPTURE_STEPS, ['announce', 'handshake', 'ext']));
 test('KEEPALIVE_DEFAULT = 90', () => assertEq(KEEPALIVE_DEFAULT, 90));

 // detectKeyFormat
 test("detectKeyFormat('key=ABC123') → 'upper_hex'", () =>
 assertEq(detectKeyFormat('key=ABC123'), 'upper_hex'));
 test("detectKeyFormat('key=abc123') → 'lower_hex'", () =>
 assertEq(detectKeyFormat('key=abc123'), 'lower_hex'));
 test("detectKeyFormat('key=Ab1234') → 'upper_hex' (A-F wins over a-f)", () =>
 assertEq(detectKeyFormat('key=Ab1234'), 'upper_hex'));
 test("detectKeyFormat('key=12345678') → null (all digits, ambiguous)", () =>
 assertEq(detectKeyFormat('key=12345678'), null));
 test("detectKeyFormat('key=xyz1234') → 'base62' (contains g-z)", () =>
 assertEq(detectKeyFormat('key=xyz1234'), 'base62'));
 test('detectKeyFormat(null) → null', () => assertEq(detectKeyFormat(null), null));
 test("detectKeyFormat('') → null", () => assertEq(detectKeyFormat(''), null));
 test("detectKeyFormat matches key as the first param (no leading &)", () =>
 assertEq(detectKeyFormat('key=ABCDEF12&numwant=200'), 'upper_hex'));
 test("detectKeyFormat matches key after another param", () =>
 assertEq(detectKeyFormat('info_hash=x&key=ab12&numwant=200'), 'lower_hex'));

 // compareVersions
 test("compareVersions('5.2.2','5.2.1') === 1", () => assertEq(compareVersions('5.2.2', '5.2.1'), 1));
 test("compareVersions('5.2.0','5.2.1') === -1", () => assertEq(compareVersions('5.2.0', '5.2.1'), -1));
 test("compareVersions('5.2.2','5.2.2') === 0", () => assertEq(compareVersions('5.2.2', '5.2.2'), 0));
 test("compareVersions('6.0','5.9.9') === 1 (major wins)", () => assertEq(compareVersions('6.0', '5.9.9'), 1));
 test("compareVersions('5.2','5.2.0') === 0 (missing segment treated as 0)", () =>
 assertEq(compareVersions('5.2', '5.2.0'), 0));

 // versionHint
 test('versionHint(1) mentions "newer"', () => assert(/newer/.test(versionHint(1))));
 test('versionHint(-1) mentions "older"', () => assert(/older/.test(versionHint(-1))));
 test('versionHint(0) === ""', () => assertEq(versionHint(0), ''));

 // reconstructCaptureQuery
 const q = 'info_hash=HHH&peer_id=PPPP&port=6881&uploaded=0&downloaded=0&left=1024&key=KEY&numwant=200&ipv4=1.2.3.4&ipv6=::1';
 test('reconstructCaptureQuery strips ipv4 and ipv6 params', () => {
 const r = reconstructCaptureQuery(q);
 assert(!/ipv4=/.test(r), 'ipv4 stripped');
 assert(!/ipv6=/.test(r), 'ipv6 stripped');
 });
 test('reconstructCaptureQuery replaces info_hash value with {info_hash}', () =>
 assert(/(^|&)info_hash=\{info_hash\}/.test(reconstructCaptureQuery(q))));
 test('reconstructCaptureQuery replaces peer_id value with {peer_id}', () =>
 assert(/(^|&)peer_id=\{peer_id\}/.test(reconstructCaptureQuery(q))));
 test('reconstructCaptureQuery replaces key value with {key}', () =>
 assert(/(^|&)key=\{key\}/.test(reconstructCaptureQuery(q))));
 test('reconstructCaptureQuery keeps static params verbatim (compact=1)', () =>
 assert(/(^|&)compact=1/.test(reconstructCaptureQuery('info_hash=H&compact=1'))));
 test('reconstructCaptureQuery("") → ""', () => assertEq(reconstructCaptureQuery(''), ''));
});
