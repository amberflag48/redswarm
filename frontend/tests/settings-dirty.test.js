import { test, suite, assert, withFixture } from './harness.js';
import { state } from '../js/state/store.js';
import { snapshotForm } from '../js/utils/form.js';
import { defaultClient } from '../js/data/client-schema.js';
import {
  renderSettingsClients, removeSettingsClient, addSettingsClient,
  settingsIsDirty,
} from '../js/components/settings-modal.js';
import { resolveConfirm } from '../js/components/confirm.js';

// Regression tests for settings-modal dirty-state detection after
// adding/removing clients. The bug: removeSettingsClient re-snapshotted
// the form AFTER removing a client card, erasing the dirty state so the
// Save button stayed disabled and the deletion never persisted.
// addSettingsClient had the same omission - no updateSaveButton() call.

const fixture = `
  <div id="settings-content">
    <div id="settings-clients"></div>
  </div>
  <button id="save-settings-btn" disabled></button>
  <div id="confirm-modal" class="hidden">
    <div id="confirm-title"></div>
    <div id="confirm-message"></div>
    <div id="confirm-buttons"></div>
  </div>
`;

suite('settings-dirty', () => {
  test('removeSettingsClient leaves the form dirty (regression)', () =>
    withFixture(fixture, async () => {
      state.settingsModalOpen = true;
      state.settingsConfig = { clients: [defaultClient(), defaultClient()] };
      renderSettingsClients();
      state.settingsFormSnapshot = snapshotForm(document.getElementById('settings-content'));
      assert(!settingsIsDirty(), 'should not be dirty before changes');

      const p = removeSettingsClient(0);
      resolveConfirm('ok');
      await p;

      assert(settingsIsDirty(), 'should be dirty after removing a client');
    }));

  test('addSettingsClient leaves the form dirty (regression)', () =>
    withFixture(fixture, () => {
      state.settingsModalOpen = true;
      state.settingsConfig = { clients: [defaultClient()] };
      renderSettingsClients();
      state.settingsFormSnapshot = snapshotForm(document.getElementById('settings-content'));
      assert(!settingsIsDirty(), 'should not be dirty before changes');

      addSettingsClient();

      assert(settingsIsDirty(), 'should be dirty after adding a client');
    }));
});
