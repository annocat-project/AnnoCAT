import assert from 'node:assert/strict';
import test from 'node:test';

import { compareNaturalResultOrder, createNaturalResultOrder } from '../src/app/result-ordering.js';

test('canonical result order does not depend on the order returned by a sorted query', () => {
  const rows = [
    { alleleId: 'third', recordNumber: 20, altIndex: 1 },
    { alleleId: 'second', recordNumber: 10, altIndex: 2 },
    { alleleId: 'first', recordNumber: 10, altIndex: 1 },
  ];
  const order = createNaturalResultOrder(rows);

  rows.sort((left, right) => compareNaturalResultOrder(order, left, right));

  assert.deepEqual(rows.map(row => row.alleleId), ['first', 'second', 'third']);
});

test('rows without canonical coordinates retain a stable fallback order', () => {
  const rows = [{ alleleId: 'second' }, { alleleId: 'first' }];
  const order = createNaturalResultOrder(rows);

  rows.sort((left, right) => compareNaturalResultOrder(order, left, right));

  assert.deepEqual(rows.map(row => row.alleleId), ['second', 'first']);
});
