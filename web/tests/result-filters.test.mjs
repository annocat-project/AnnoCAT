import assert from 'node:assert/strict';
import test from 'node:test';

import { remapEvidenceFilterRules } from '../src/app/result-filters.js';

test('annotation filters keep their field identity when the catalog changes', () => {
  const phenotype = { scope: 'gene', sourceId: 'hpo', fieldPath: 'phenotypeRelevance' };
  const proteinFunction = { scope: 'allele', sourceId: 'favor-online', fieldPath: 'apcProteinFunction' };
  const rules = [
    { column: 'evidence:0', operator: 'gte', value: '50' },
    { column: 'gene', operator: 'in', value: 'CACNA1A' },
  ];

  assert.deepEqual(
    remapEvidenceFilterRules(rules, [phenotype], [proteinFunction, phenotype]),
    [
      { column: 'evidence:1', operator: 'gte', value: '50' },
      { column: 'gene', operator: 'in', value: 'CACNA1A' },
    ],
  );
});

test('annotation filters are removed instead of changing meaning when a field disappears', () => {
  const rules = [{ column: 'evidence:0', operator: 'gte', value: '50' }];
  const phenotype = { scope: 'gene', sourceId: 'hpo', fieldPath: 'phenotypeRelevance' };

  assert.deepEqual(remapEvidenceFilterRules(rules, [phenotype], []), []);
});
