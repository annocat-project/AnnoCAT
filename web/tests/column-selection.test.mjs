import assert from 'node:assert/strict';
import test from 'node:test';

import { shouldSelectColumnGroup } from '../src/app/column-selection.js';

test('column group toggles select, clear, and recover from a capped partial selection', () => {
  assert.equal(shouldSelectColumnGroup([{ selected: false, evidence: true }], 0), true);
  assert.equal(shouldSelectColumnGroup([{ selected: true, evidence: true }], 1), false);
  assert.equal(shouldSelectColumnGroup([
    { selected: true, evidence: true },
    { selected: false, evidence: true },
  ], 32), false);
});
