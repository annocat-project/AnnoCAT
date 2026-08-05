import assert from 'node:assert/strict';
import test from 'node:test';
import {
  PROFILE_EVIDENCE_DEPENDENCIES,
  matchedGeneCountText,
  profileEvidenceDependencyIndexes,
  summarizeProfileEvidence,
  summarizeProfileEvidenceRow,
} from '../src/app/phenotypes.js';
import { applyGenericEvidenceCellPresentation } from '../src/app/variant-presentation.js';

globalThis.localStorage = { getItem: () => null, setItem: () => {} };

test('matched gene totals use compact popover copy', () => {
  assert.equal(matchedGeneCountText(undefined), '');
  assert.equal(matchedGeneCountText(0), '0 genes found. ');
  assert.equal(matchedGeneCountText(1), '1 gene found. ');
  assert.equal(matchedGeneCountText(1200), '1,200 genes found. ');
});

test('condition details populate the composite phenotype cell when no score is reported', () => {
  assert.ok(PROFILE_EVIDENCE_DEPENDENCIES.includes('phenotypeEvidenceDetails'));
  const summary = summarizeProfileEvidence({
    score: 'Not reported',
    details: {
      conditionLinks: [{
        selectedConditionId: 'MONDO:0005277',
        selectedCondition: 'migraine disorder',
        relation: 'Condition subtype',
      }],
    },
  });
  assert.equal(summary.score, 'migraine disorder');
  assert.equal(summary.secondary, 'Condition subtype');
  assert.equal(summary.display, 'migraine disorder');
  assert.match(summary.tooltip, /MONDO:0005277 migraine disorder/);
  assert.match(summary.tooltip, /Relation: Condition subtype/);
});

test('phenotype and condition evidence share one compact display line', () => {
  const summary = summarizeProfileEvidence({
    score: '100.0',
    conditionMatches: 1,
    matchedConditions: 'MONDO:0005277 migraine disorder',
    conditionRelation: 'Condition subtype',
  });
  assert.equal(summary.display, '100.0 · migraine disorder');
  assert.match(summary.tooltip, /Relation: Condition subtype/);
});

test('composite phenotype evidence uses the populated active dependency', () => {
  const catalog = [
    { sourceId: 'hpo', fieldPath: 'phenotypeRelevance' },
    { sourceId: 'hpo', fieldPath: 'selectedConditionMatches' },
    { sourceId: 'hpo@2026', fieldPath: 'selectedConditionMatches' },
    { sourceId: 'hpo@2026', fieldPath: 'matchedSelectedConditions' },
    { sourceId: 'hpo@2026', fieldPath: 'selectedConditionRelation' },
  ];
  assert.deepEqual(profileEvidenceDependencyIndexes(catalog, 0), [1, 2, 3, 4]);
  const summary = summarizeProfileEvidenceRow({
    catalog,
    rowEvidence: {
      2: '1',
      3: 'MONDO:0005277 migraine disorder',
      4: 'Condition subtype',
    },
    index: 0,
    score: 'Not reported',
  });
  assert.equal(summary.score, 'migraine disorder');
  assert.equal(summary.secondary, 'Condition subtype');
});

test('generic evidence styling preserves a composite phenotype cell', () => {
  const cell = {
    innerHTML: '<span class="profile-evidence-cell">migraine disorder</span>',
    querySelector: selector => selector === '.profile-evidence-cell' ? {} : null,
  };
  assert.equal(
    applyGenericEvidenceCellPresentation(cell, '<span>Not reported</span>'),
    false,
  );
  assert.match(cell.innerHTML, /migraine disorder/);
});
