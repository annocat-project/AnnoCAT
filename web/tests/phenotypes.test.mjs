import assert from 'node:assert/strict';
import test from 'node:test';
import {
  PROFILE_EVIDENCE_DEPENDENCIES,
  phenotypeRankDependencyIndexes,
  summarizeGeneMatchRow,
  summarizePhenotypeRankRow,
  profileEvidenceDependencyIndexes,
  summarizeProfileEvidence,
  summarizeProfileEvidenceRow,
  splitGeneListEntries,
  formatGeneListSections,
  summarizeGenePreviewScope,
  POLYGENIC_ASSOCIATIONS_TOOLTIP,
  UPSTREAM_DOWNSTREAM_TOOLTIP,
  PHENOTYPE_SEARCH_PENDING,
  PHENOTYPE_SEARCH_EMPTY,
  activeGeneSettingCount,
} from '../src/app/phenotypes.js';
import { applyGenericEvidenceCellPresentation, evidenceColumnPolicy } from '../src/app/variant-presentation.js';

globalThis.localStorage = { getItem: () => null, setItem: () => {} };

test('pasted gene lists accept commas, spaces, and lines without duplicates', () => {
  assert.deepEqual(
    splitGeneListEntries('BRCA1, BRCA2\nENSG00000141510  BRCA1'),
    ['BRCA1', 'BRCA2', 'ENSG00000141510'],
  );
});

test('labeled gene-list sections remain editable and resolve only their genes', () => {
  const text = formatGeneListSections([
    { label: 'Steroid pathway', genes: [{ label: 'SRD5A2' }, { label: 'CYP21A2' }] },
    { label: 'Migraine', genes: [{ label: 'CACNA1A' }, { label: 'ATP1A2' }] },
  ]);
  assert.equal(
    text,
    '[Steroid pathway]\nSRD5A2, CYP21A2\n\n[Migraine]\nCACNA1A, ATP1A2',
  );
  assert.deepEqual(
    splitGeneListEntries(text),
    ['SRD5A2', 'CYP21A2', 'CACNA1A', 'ATP1A2'],
  );
});

test('generated source sections keep typed headings, including empty sections', () => {
  const text = formatGeneListSections([
    {
      label: 'Feature: Migraine (HP:0002076) · MENDELIAN',
      genes: [{ symbol: 'CACNA1A' }, { symbol: 'ATP1A2' }],
      alwaysHeading: true,
    },
    {
      label: 'Feature: Migraine (HP:0002076) · POLYGENIC',
      genes: [],
      alwaysHeading: true,
    },
  ]);
  assert.equal(
    text,
    '[Feature: Migraine (HP:0002076) · MENDELIAN]\nCACNA1A, ATP1A2\n\n[Feature: Migraine (HP:0002076) · POLYGENIC]\n',
  );
  assert.deepEqual(splitGeneListEntries(text), ['CACNA1A', 'ATP1A2']);
});

test('zero-overlap previews keep the missing-genes action but cannot apply', () => {
  const scope = summarizeGenePreviewScope({
    includedGenes: 3,
    includedGenesInResult: 0,
    genesInResult: 20,
  });
  assert.equal(scope.canApply, false);
  assert.match(scope.html, /None of the 3 associated genes have variants in this result/);
  assert.match(scope.html, /data-view-missing-genes/);
  assert.match(scope.html, /View 3 without variants/);

  const partial = summarizeGenePreviewScope({
    includedGenes: 3,
    includedGenesInResult: 1,
    genesInResult: 20,
  });
  assert.equal(partial.canApply, true);
  assert.match(partial.html, /View 2 without variants/);
});

test('empty biological resolution is distinct from zero result overlap', () => {
  const scope = summarizeGenePreviewScope({
    includedGenes: 0,
    includedGenesInResult: 0,
    genesInResult: 20,
  });
  assert.equal(scope.canApply, false);
  assert.equal(
    scope.html,
    'No associated genes were found for this selection in the installed HPO/MONDO data.',
  );
  assert.doesNotMatch(scope.html, /data-view-missing-genes/);
});

test('approved scope controls and listbox states retain exact user-facing copy', () => {
  assert.equal(PHENOTYPE_SEARCH_PENDING, 'Searching…');
  assert.equal(
    PHENOTYPE_SEARCH_EMPTY,
    'No matching feature, condition, pathway, or gene',
  );
  assert.match(POLYGENIC_ASSOCIATIONS_TOOLTIP, /Mendelian disease-gene associations/);
  assert.match(UPSTREAM_DOWNSTREAM_TOOLTIP, /within 5 kb/);
  assert.match(UPSTREAM_DOWNSTREAM_TOOLTIP, /proximity alone does not show/);
  assert.match(UPSTREAM_DOWNSTREAM_TOOLTIP, /Transcript and evidence selection/);
  assert.doesNotMatch(UPSTREAM_DOWNSTREAM_TOOLTIP, /—/);
});

test('gene settings report how many scope options are enabled', () => {
  assert.equal(activeGeneSettingCount(), 0);
  assert.equal(activeGeneSettingCount({ includePolygenic: true }), 1);
  assert.equal(activeGeneSettingCount({ includeUpstreamDownstream: true }), 1);
  assert.equal(activeGeneSettingCount({
    includePolygenic: true,
    includeUpstreamDownstream: true,
  }), 2);
});

test('gene matches use one compact value with detailed provenance', () => {
  const catalog = [
    {
      sourceId: 'gene-profile',
      fieldPath: 'geneMatches',
      presentationDependencies: ['geneMatchDetails'],
    },
    { sourceId: 'gene-profile', fieldPath: 'geneMatchDetails' },
  ];
  const summary = summarizeGeneMatchRow({
    catalog,
    rowEvidence: {
      1: [{
        selectedItemId: 'HP:0002076',
        selectedItem: 'Migraine',
        itemType: 'Feature',
        geneSymbol: 'CACNA1A',
        relation: 'HPO link via exact disease annotation',
      }],
    },
    index: 0,
    value: 'Migraine',
  });
  assert.equal(summary.display, 'Migraine');
  assert.equal(summary.tooltip, 'HP:0002076 Migraine · Feature · CACNA1A · HPO link via exact disease annotation');
});

test('proximity gene matches explain the matching transcript and representative row gene', () => {
  const catalog = [
    {
      sourceId: 'gene-profile',
      fieldPath: 'geneMatches',
      presentationDependencies: ['geneMatchDetails'],
    },
    { sourceId: 'gene-profile', fieldPath: 'geneMatchDetails' },
  ];
  const summary = summarizeGeneMatchRow({
    catalog,
    rowEvidence: {
      1: [{
        selectedItemId: 'Gene:CACNA1A',
        selectedItem: 'CACNA1A',
        itemType: 'Gene',
        geneSymbol: 'CACNA1A',
        representativeGene: 'ITPR1',
        consequence: 'downstream_gene_variant',
        relation: 'Entered gene',
      }],
    },
    index: 0,
    value: 'CACNA1A',
  });
  assert.match(summary.tooltip, /Matched gene: CACNA1A/);
  assert.match(summary.tooltip, /Consequence: downstream gene variant/);
  assert.match(summary.tooltip, /representative row gene is ITPR1/);
  assert.match(summary.tooltip, /select the CACNA1A transcript/);
});

test('phenotype rank is relative, exposes ties, and explains Resnik without implying probability', () => {
  const catalog = [
    {
      sourceId: 'gene-profile',
      fieldPath: 'phenotypeRank',
      presentationDependencies: ['phenotypeRankDetails'],
    },
    { sourceId: 'gene-profile', fieldPath: 'phenotypeRankDetails' },
  ];
  assert.deepEqual(phenotypeRankDependencyIndexes(catalog, 0), [1]);
  const summary = summarizePhenotypeRankRow({
    catalog,
    rowEvidence: {
      1: {
        rank: 3,
        denominator: 4804,
        tieCount: 2,
        queryTermCount: 1,
        geneSymbol: 'CACNA1A',
        bestDisease: 'Episodic ataxia type 2',
        bestDiseaseId: 'OMIM:108500',
        hpoRelease: '2026-07-24',
      },
    },
    index: 0,
    value: 3,
  });
  assert.equal(summary.display, '3 of 4,804 · 2 tied');
  assert.match(summary.tooltip, /broad features may produce many ties/);
  assert.match(summary.tooltip, /Resnik query-to-disease best-match average/);
  assert.match(summary.tooltip, /not a diagnostic probability/);
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

test('legacy phenotype score and helper fields remain audit-only', () => {
  for (const sourceId of ['hpo', 'gene-profile']) {
    for (const fieldPath of ['phenotypeRelevance', 'geneMatch', 'absentFeatureConflict']) {
      assert.deepEqual(
        evidenceColumnPolicy({ sourceId, fieldPath }),
        { selectable: false, recommended: false },
      );
    }
  }
});

test('catalog recommendation controls corrected phenotype column defaults', () => {
  assert.deepEqual(
    evidenceColumnPolicy({
      sourceId: 'gene-profile',
      fieldPath: 'phenotypeRank',
      selectable: true,
      recommended: true,
    }),
    { selectable: true, recommended: true },
  );
  assert.deepEqual(
    evidenceColumnPolicy({
      sourceId: 'gene-profile',
      fieldPath: 'phenotypeRank',
      selectable: true,
      recommended: false,
    }),
    { selectable: true, recommended: false },
  );
});
