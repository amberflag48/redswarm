import { test, suite, assert, assertEq, withFixture } from './harness.js';
import {
 CLIENT_FIELDS, clientFieldId, defaultClient,
 collectOneClient, parseMDict, mDictToText,
} from '../js/data/client-schema.js';

suite('client-schema', () => {
 // defaultClient
 test('CLIENT_FIELDS declares exactly 18 fields', () => assertEq(CLIENT_FIELDS.length, 18));
 test('defaultClient has all 18 keys', () => {
 const c = defaultClient();
 assertEq(Object.keys(c).length, 18, 'expected 18 client fields');
 });
 test('defaultClient: every CLIENT_FIELDS key is present with its declared default', () => {
 const c = defaultClient();
 for (const f of CLIENT_FIELDS) {
 assert(Object.prototype.hasOwnProperty.call(c, f.key), `missing key ${f.key}`);
 assertEq(c[f.key], f.default, `default mismatch for ${f.key}`);
 }
 });
 test('defaultClient defaults: label="", version="", fast_extension=false, key_format="upper_hex"', () => {
 const c = defaultClient();
 assertEq(c.label, '');
 assertEq(c.version, '');
 assertEq(c.fast_extension, false);
 assertEq(c.key_format, 'upper_hex');
 });

 // clientFieldId
 test("clientFieldId(0, 'label') === 'set-client-0-label'", () =>
 assertEq(clientFieldId(0, 'label'), 'set-client-0-label'));
 test("clientFieldId('cap', 'numwant') === 'set-client-cap-numwant'", () =>
 assertEq(clientFieldId('cap', 'numwant'), 'set-client-cap-numwant'));

 // parseMDict
 test('parseMDict parses two entries', () =>
 assertEq(parseMDict('ut_pex = 1\nut_metadata = 2'), { ut_pex: 1, ut_metadata: 2 }));
 test('parseMDict("") → {}', () => assertEq(parseMDict(''), {}));
 test('parseMDict skips invalid lines', () =>
 assertEq(parseMDict('invalid line\nut_pex = 1'), { ut_pex: 1 }));
 test('parseMDict ignores non-numeric values', () =>
 assertEq(parseMDict('ut_pex = abc\nut_holepunch = 5'), { ut_holepunch: 5 }));

 // mDictToText
 test('mDictToText serialises entries as "k = v"', () => {
 const text = mDictToText({ a: 1, b: 2 });
 assert(text.includes('a = 1'), 'contains "a = 1"');
 assert(text.includes('b = 2'), 'contains "b = 2"');
 });
 test('mDictToText(null) → ""', () => assertEq(mDictToText(null), ''));
 test('mDictToText({}) → ""', () => assertEq(mDictToText({}), ''));

 // collectOneClient (failure path)
 test('collectOneClient returns null when no matching card exists', () =>
 withFixture('<div></div>', () => assertEq(collectOneClient(0), null)));
});
