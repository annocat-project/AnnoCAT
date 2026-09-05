import { requestFluentText } from './ui-components.js';

export const PROFILE_EVIDENCE_DEPENDENCIES = [
  'selectedConditionMatches',
  'matchedSelectedConditions',
  'selectedConditionRelation',
  'directFeatureMatches',
  'absentFeatureConflict',
  'phenotypeEvidenceDetails',
];

export function geneMatchDependencyIndexes(catalog, index) {
  const field = catalog[index] || {};
  const dependencies = new Set(field.presentationDependencies || []);
  return catalog.flatMap((candidate, candidateIndex) =>
    dependencies.has(candidate?.fieldPath) && candidate?.sourceId === field.sourceId
      ? [candidateIndex]
      : [],
  );
}

export function summarizeGeneMatchRow({
  catalog,
  rowEvidence,
  index,
  value,
  decode = item => item,
}) {
  const dependencies = geneMatchDependencyIndexes(catalog, index);
  const dependencyValue = path => {
    for (const dependency of dependencies) {
      if (catalog[dependency]?.fieldPath !== path) continue;
      const item = decode(rowEvidence?.[dependency]);
      if (item !== null && item !== undefined && item !== '') return item;
    }
    return null;
  };
  let details = dependencyValue('geneMatchDetails');
  if (typeof details === 'string') {
    try { details = JSON.parse(details); } catch { details = null; }
  }
  const matches = Array.isArray(details) ? details : [];
  const tooltip = matches.length
    ? matches.map(match => {
      const provenance = [
        [match.selectedItemId, match.selectedItem].filter(Boolean).join(' '),
        match.itemType,
        match.geneSymbol,
        match.relation,
      ].filter(Boolean).join(' · ');
      const proximity = ['upstream_gene_variant', 'downstream_gene_variant']
        .includes(match.consequence);
      if (!proximity) return provenance;
      const consequence = match.consequence.replaceAll('_', ' ');
      const representative = match.representativeGene &&
        match.representativeGene.toUpperCase() !== match.geneSymbol?.toUpperCase()
        ? ` The representative row gene is ${match.representativeGene}.`
        : '';
      return `${provenance}. Matched gene: ${match.geneSymbol}. Consequence: ${consequence}.${representative} Open Variant Details and select the ${match.geneSymbol} transcript.`;
    }).join('\n')
    : [dependencyValue('matchedSelectedItems'), dependencyValue('matchedItemTypes')]
      .filter(Boolean).join(' · ');
  return {
    display: value && value !== 'Not reported' ? String(value) : 'No match',
    tooltip: tooltip || 'No selected item matches this variant gene.',
  };
}

export function phenotypeRankDependencyIndexes(catalog, index) {
  const field = catalog[index] || {};
  const dependencies = new Set(field.presentationDependencies || []);
  return catalog.flatMap((candidate, candidateIndex) =>
    dependencies.has(candidate?.fieldPath) && candidate?.sourceId === field.sourceId
      ? [candidateIndex]
      : [],
  );
}

export function summarizePhenotypeRankRow({
  catalog,
  rowEvidence,
  index,
  value,
  decode = item => item,
}) {
  const detailsIndex = phenotypeRankDependencyIndexes(catalog, index)
    .find(candidate => catalog[candidate]?.fieldPath === 'phenotypeRankDetails');
  let details = detailsIndex === undefined ? null : decode(rowEvidence?.[detailsIndex]);
  if (typeof details === 'string') {
    try { details = JSON.parse(details); } catch { details = null; }
  }
  const rank = Number(value ?? details?.rank);
  const denominator = Number(details?.denominator);
  if (!Number.isInteger(rank) || rank < 1 || !Number.isInteger(denominator) || denominator < 1) {
    return {
      display: 'Not ranked',
      tooltip: 'No eligible HPO disease profile was available for this gene.',
    };
  }
  const ties = Math.max(1, Number(details?.tieCount) || 1);
  const featureCount = Math.max(0, Number(details?.queryTermCount) || 0);
  const gene = details?.geneSymbol || 'Variant gene';
  const featureText = featureCount === 1
    ? 'Based on 1 selected HPO feature; broad features may produce many ties.'
    : `Based on ${featureCount.toLocaleString()} selected HPO features.`;
  const disease = details?.bestDisease
    ? `Best-matching disease: ${details.bestDisease}${details.bestDiseaseId ? ` (${details.bestDiseaseId})` : ''}.`
    : '';
  const tied = ties > 1 ? `${ties.toLocaleString()} genes share this rank.` : '';
  const release = details?.hpoRelease ? `HPO release: ${details.hpoRelease}.` : '';
  return {
    display: `${rank.toLocaleString()} of ${denominator.toLocaleString()}${ties > 1 ? ` · ${ties.toLocaleString()} tied` : ''}`,
    tooltip: [
      `${gene} — Rank ${rank.toLocaleString()} of ${denominator.toLocaleString()}${ties > 1 ? ` · ${ties.toLocaleString()} tied` : ''}.`,
      featureText,
      disease,
      tied,
      'Method: Resnik query-to-disease best-match average.',
      release,
      'This is a relative phenotype-similarity rank. It is not a diagnostic probability, pathogenicity classification, or the reason this gene was included.',
    ].filter(Boolean).join(' '),
  };
}

function hpoField(field) {
  const source = String(field?.sourceId || '').toLowerCase();
  return source === 'hpo' || source.startsWith('hpo-') || source.startsWith('hpo@');
}

export function profileEvidenceDependencyIndexes(catalog, index) {
  const field = catalog[index] || {};
  const dependencies = new Set([
    ...(Array.isArray(field.presentationDependencies)
      ? field.presentationDependencies
      : []),
    ...PROFILE_EVIDENCE_DEPENDENCIES,
  ]);
  return catalog.flatMap((candidate, candidateIndex) =>
    dependencies.has(candidate?.fieldPath) &&
    (candidate?.sourceId === field.sourceId || hpoField(field) && hpoField(candidate))
      ? [candidateIndex]
      : [],
  );
}

export function summarizeProfileEvidence({
  score = 'Not reported',
  conditionMatches = null,
  matchedConditions = '',
  conditionRelation = '',
  directMatches = null,
  absentConflict = null,
  details = null,
} = {}) {
  if (typeof details === 'string') {
    try {
      details = JSON.parse(details);
    } catch {
      details = null;
    }
  }
  const links = Array.isArray(details?.conditionLinks)
    ? details.conditionLinks.filter(link => link && typeof link === 'object')
    : [];
  const detailConditions = [...new Set(links.map(link =>
    [link.selectedConditionId, link.selectedCondition].filter(Boolean).join(' '),
  ).filter(Boolean))].join('; ');
  const numericConditions = Number(conditionMatches);
  const conditions = Math.max(
    Number.isFinite(numericConditions) ? numericConditions : 0,
    links.length,
  );
  const numericDirect = Number(directMatches);
  const direct = Number.isFinite(numericDirect) ? numericDirect : 0;
  matchedConditions = String(matchedConditions || detailConditions);
  conditionRelation = String(conditionRelation ||
    (links.some(link => link.relation === 'Exact condition')
      ? 'Exact condition'
      : links.find(link => link.relation)?.relation || ''));
  const conditionLabels = matchedConditions.split(';')
    .map(item => item.trim().replace(/^MONDO:\d+\s+/i, ''))
    .filter(Boolean);
  const conditionLabel = conditionLabels.length > 1
    ? `${conditionLabels[0]} +${conditionLabels.length - 1}`
    : conditionLabels[0] || '';
  const hasScore = Boolean(score && score !== 'Not reported');
  const primary = hasScore
    ? score
    : conditions > 0
      ? conditionLabel || 'Condition match'
      : 'Not reported';
  const secondary = conditionLabel && primary !== conditionLabel
    ? conditionLabel
    : conditionRelation || (direct > 0
      ? `${direct} direct feature ${direct === 1 ? 'match' : 'matches'}`
      : '');
  const displaySecondary = primary === conditionLabel ? '' : secondary;
  const parts = [];
  if (hasScore) parts.push(`Phenotype relevance: ${score}`);
  if (conditions > 0) parts.push(`${conditions} selected condition ${conditions === 1 ? 'link' : 'links'}`);
  if (direct > 0) parts.push(`${direct} direct feature ${direct === 1 ? 'match' : 'matches'}`);
  if (matchedConditions) parts.push(`Matched condition: ${matchedConditions}`);
  if (conditionRelation) parts.push(`Relation: ${conditionRelation}`);
  if (absentConflict !== null && absentConflict !== undefined && Number(absentConflict) > 0) {
    parts.push(`Absent-feature conflict: ${absentConflict}`);
  }
  return {
    score: primary,
    secondary,
    conditionLabel,
    conditionRelation,
    conditions,
    direct,
    display: `${primary}${displaySecondary ? ` · ${displaySecondary}` : ''}`,
    tooltip: parts.join('. ') || 'No phenotype evidence was reported for this gene.',
  };
}

export function summarizeProfileEvidenceRow({
  catalog,
  rowEvidence,
  index,
  score,
  decode = value => value,
}) {
  const dependencies = profileEvidenceDependencyIndexes(catalog, index);
  const dependencyValue = path => {
    for (const dependency of dependencies) {
      if (catalog[dependency]?.fieldPath !== path) continue;
      const value = decode(rowEvidence?.[dependency]);
      if (value !== null && value !== undefined && value !== '') return value;
    }
    return null;
  };
  return summarizeProfileEvidence({
    score,
    conditionMatches: dependencyValue('selectedConditionMatches'),
    matchedConditions: dependencyValue('matchedSelectedConditions'),
    conditionRelation: dependencyValue('selectedConditionRelation'),
    directMatches: dependencyValue('directFeatureMatches'),
    absentConflict: dependencyValue('absentFeatureConflict'),
    details: dependencyValue('phenotypeEvidenceDetails'),
  });
}

export function splitGeneListEntries(value) {
  const withoutSectionHeadings = String(value || '')
    .split(/\r?\n/)
    .filter(line => !/^\s*\[[^\]]+\]\s*$/.test(line))
    .join('\n');
  return [...new Set(withoutSectionHeadings
    .split(/[\s,]+/)
    .map(entry => entry.trim())
    .filter(Boolean))];
}

export function formatGeneListSections(sections) {
  const populated = sections
    .map(section => ({
      label: String(section.label || 'Gene list').replace(/[\[\]\r\n]+/g, ' ').trim(),
      genes: section.genes || [],
      alwaysHeading: Boolean(section.alwaysHeading),
    }))
    .filter(section => section.genes.length || section.alwaysHeading);
  if (populated.length <= 1 && !populated[0]?.alwaysHeading) {
    return populated[0]?.genes.map(gene => gene.symbol || gene.label).join(', ') || '';
  }
  return populated.map(section =>
    `[${section.label}]\n${section.genes.map(gene => gene.symbol || gene.label).join(', ')}`,
  ).join('\n\n');
}

export const POLYGENIC_ASSOCIATIONS_TOOLTIP = 'By default, HPO features and MONDO conditions include only Mendelian disease-gene associations. Turn this on to also include associations labeled POLYGENIC. Mendelian associations remain included.';
export const UPSTREAM_DOWNSTREAM_TOOLTIP = "Off by default. Includes VEP upstream/downstream matches within 5 kb of a transcript for a selected gene, which is VEP's default distance. These variants can be biologically relevant, but proximity alone does not show that they affect the selected gene. A row may display a different representative gene. Open Variant Details and use the transcript selector to view the selected gene's upstream/downstream annotation. See Transcript and evidence selection in the documentation.";
export const PHENOTYPE_SEARCH_PENDING = 'Searching…';
export const PHENOTYPE_SEARCH_EMPTY = 'No matching feature, condition, pathway, or gene';

export function activeGeneSettingCount(profile = {}) {
  return Number(Boolean(profile.includePolygenic)) +
    Number(Boolean(profile.includeUpstreamDownstream));
}

export function summarizeGenePreviewScope(preview) {
  if (!preview) return { canApply: false, html: '' };
  if (preview.includedGenes === 0) {
    return {
      canApply: false,
      html: 'No associated genes were found for this selection in the installed HPO/MONDO data.',
    };
  }
  const includedInResult = preview.includedGenesInResult ??
    Math.min(preview.includedGenes, preview.genesInResult);
  const withoutVariants = Math.max(0, preview.includedGenes - includedInResult);
  const viewMissing = withoutVariants
    ? ` <button type="button" class="fui-button fui-button--subtle" data-view-missing-genes>View ${withoutVariants.toLocaleString()} without variants</button>`
    : '';
  return {
    canApply: includedInResult > 0,
    html: includedInResult === 0
      ? `None of the ${preview.includedGenes.toLocaleString()} associated ${preview.includedGenes === 1 ? 'gene has' : 'genes have'} variants in this result.${viewMissing}`
      : `${includedInResult.toLocaleString()} of ${preview.includedGenes.toLocaleString()} associated ${preview.includedGenes === 1 ? 'gene has' : 'genes have'} variants in this result.${viewMissing}`,
  };
}

export function createPhenotypeFeature({
  $,
  escapeHtml,
  prototypeIcon,
  showPage,
  onApply,
}) {
  let run = null;
  let resources = {};
  let profile = emptyProfile();
  let results = [];
  let activeIndex = -1;
  let timer = null;
  let request = null;
  let searchText = '';
  let searchLoading = false;
  let searchComplete = false;
  let searchAnnouncement = '';
  let geneSettingsOpen = false;
  let message = '';
  let applying = false;
  let applyStartedAt = 0;
  let applyElapsedTimer = null;
  let preview = null;
  let previewLoading = false;
  let previewError = '';
  let previewTimer = null;
  let previewRequest = null;
  let previewRevision = 0;
  let resolutionAnnouncementTimer = null;
  let pasteTimer = null;
  let pasteRequest = null;
  let pasteRevision = 0;
  let pasteText = '';
  let pasteResolution = null;
  let pasteLoading = false;
  let savedGeneLists = [];
  let selectedGeneListName = '';
  let geneListDraft = [];
  let geneSections = [];
  let missingGenes = [];
  let missingGenesHasMore = false;
  let missingGenesRequest = null;
  let missingGenesTimer = null;
  let missingGenesInvoker = null;
  let profileLoadError = '';

  function resetDraftState() {
    clearTimeout(timer);
    clearTimeout(previewTimer);
    clearTimeout(pasteTimer);
    clearTimeout(resolutionAnnouncementTimer);
    request?.abort();
    previewRequest?.abort();
    pasteRequest?.abort();
    request = null;
    previewRequest = null;
    pasteRequest = null;
    pasteLoading = false;
    profile = emptyProfile();
    message = '';
    results = [];
    searchText = '';
    searchLoading = false;
    searchComplete = false;
    searchAnnouncement = '';
    activeIndex = -1;
    preview = null;
    previewError = '';
    previewLoading = false;
    previewRevision += 1;
    pasteText = '';
    pasteResolution = null;
    geneListDraft = [];
    geneSections = [];
    selectedGeneListName = '';
    profileLoadError = '';
    pasteRevision += 1;
  }

  function emptyProfile() {
    return {
      observed: [],
      conditions: [],
      pathways: [],
      genes: [],
      includePolygenic: false,
      includeUpstreamDownstream: false,
      showMatchesOnly: false,
      mondoRelease: null,
    };
  }

  function normalizeProfile(value = {}) {
    return {
      ...value,
      observed: Array.isArray(value.observed) ? value.observed : [],
      conditions: Array.isArray(value.conditions) ? value.conditions : [],
      pathways: Array.isArray(value.pathways) ? value.pathways : [],
      genes: Array.isArray(value.genes) ? value.genes : [],
      includePolygenic: Boolean(value.includePolygenic),
      includeUpstreamDownstream: Boolean(value.includeUpstreamDownstream),
      showMatchesOnly: Boolean(value.showMatchesOnly),
    };
  }

  function host() {
    let popover = $('#phenotype-popover');
    if (popover) return popover;
    document.body.insertAdjacentHTML(
      'beforeend',
      '<section id="phenotype-popover" class="phenotype-popover fui-popover fui-popover--dialog fui-popover--nested-content hidden" role="dialog" aria-labelledby="phenotype-popover-title"></section>',
    );
    popover = $('#phenotype-popover');
    popover.addEventListener('click', handleClick);
    popover.addEventListener('input', handleInput);
    popover.addEventListener('keydown', handleKeydown);
    popover.addEventListener('focusout', event => {
      if (!event.target.matches('[data-paste-genes]') || !pasteTimer) return;
      clearTimeout(pasteTimer);
      pasteTimer = null;
      void resolvePaste();
    });
    return popover;
  }

  function missingGenesDialog() {
    let dialog = $('#genes-without-variants');
    if (dialog) return dialog;
    document.body.insertAdjacentHTML(
      'beforeend',
      `<dialog id="genes-without-variants" class="fui-dialog fui-dialog--wide" aria-labelledby="genes-without-variants-title"><section class="fui-dialog__surface"><header class="fui-dialog__header"><div><h2 id="genes-without-variants-title">Genes without variants</h2><p class="fui-dialog__description">These selected genes have no variants in this result.</p></div><button type="button" class="fui-button fui-button--icon" data-close-missing-genes aria-label="Close">${prototypeIcon('close')}</button></header><div class="fui-dialog__content fui-dialog__content--scrollable"><label class="fui-field"><span class="fui-field__label">Search genes</span><input type="search" class="fui-input" data-search-missing-genes autocomplete="off"></label><div class="fui-list fui-list--divided" data-missing-gene-list></div><button type="button" class="fui-button hidden" data-load-more-missing-genes>Load more</button></div><footer class="fui-dialog__footer"><div class="fui-dialog__actions"><button type="button" class="fui-button" data-close-missing-genes>Close</button></div></footer></section></dialog>`,
    );
    dialog = $('#genes-without-variants');
    dialog.addEventListener('click', event => {
      if (event.target.closest('[data-close-missing-genes]')) dialog.close();
      if (event.target.closest('[data-load-more-missing-genes]')) {
        void loadMissingGenes({ append: true });
      }
    });
    dialog.addEventListener('input', event => {
      if (!event.target.matches('[data-search-missing-genes]')) return;
      clearTimeout(missingGenesTimer);
      missingGenesTimer = setTimeout(() => void loadMissingGenes(), 180);
    });
    dialog.addEventListener('close', () => {
      missingGenesRequest?.abort();
      missingGenesRequest = null;
      clearTimeout(missingGenesTimer);
      missingGenesInvoker?.focus();
      missingGenesInvoker = null;
    });
    return dialog;
  }

  function renderMissingGenes() {
    const dialog = missingGenesDialog();
    const list = dialog.querySelector('[data-missing-gene-list]');
    list.innerHTML = missingGenes.length
      ? missingGenes.map(gene => `<div class="fui-list-row fui-list-row--two-column"><strong>${escapeHtml(gene.symbol)}</strong><span>${escapeHtml([...new Set([gene.canonicalGeneId, gene.resultGeneId, ...(gene.sources || [])].filter(Boolean))].join(' · '))}</span></div>`).join('')
      : '<p class="fui-caption">No genes found.</p>';
    dialog.querySelector('[data-load-more-missing-genes]')
      ?.classList.toggle('hidden', !missingGenesHasMore);
  }

  async function loadMissingGenes({ append = false } = {}) {
    if (!run) return;
    missingGenesRequest?.abort();
    const controller = new AbortController();
    missingGenesRequest = controller;
    const dialog = missingGenesDialog();
    const query = dialog.querySelector('[data-search-missing-genes]')?.value.trim() || '';
    const offset = append ? missingGenes.length : 0;
    try {
      const params = new URLSearchParams({
        offset: String(offset),
        limit: '100',
        q: query,
        presence: 'not-in-result',
      });
      const response = await fetch(
        `/api/runs/${encodeURIComponent(run.id)}/genes/preview?${params}`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-AnnoCat-CSRF': '1' },
          body: JSON.stringify(draftRequest('preview')),
          signal: controller.signal,
        },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error || 'Could not load genes');
      missingGenes = append ? [...missingGenes, ...(body.rows || [])] : body.rows || [];
      missingGenesHasMore = Boolean(body.hasMore);
      renderMissingGenes();
    } catch (error) {
      if (error.name !== 'AbortError') {
        dialog.querySelector('[data-missing-gene-list]').innerHTML =
          `<p class="fui-status-message fui-status-message--error">${escapeHtml(error.message)}</p>`;
      }
    } finally {
      if (missingGenesRequest === controller) missingGenesRequest = null;
    }
  }

  function openMissingGenes() {
    const dialog = missingGenesDialog();
    missingGenesInvoker = document.activeElement;
    missingGenes = [];
    missingGenesHasMore = false;
    dialog.querySelector('[data-search-missing-genes]').value = '';
    dialog.showModal();
    void loadMissingGenes();
  }

  function terms(kind) {
    return Array.isArray(profile?.[kind]) ? profile[kind] : [];
  }

  function cleanTerms(items) {
    return items.map(({ id, label }) => ({ id, label }));
  }

  function cleanGenes(items) {
    return items.map(gene => ({
      symbol: gene.symbol || gene.label,
      canonicalGeneId: gene.canonicalGeneId || null,
      resultGeneId: gene.resultGeneId || null,
      identityStatus: gene.identityStatus,
    }));
  }

  function savedGeneValue(gene) {
    return {
      id: gene.canonicalGeneId || gene.resultGeneId || gene.id || gene.symbol || gene.label,
      label: gene.symbol || gene.label,
    };
  }

  function hasPositiveInput() {
    return Boolean(
      terms('observed').length ||
      terms('conditions').length ||
      terms('pathways').length ||
      terms('genes').length
    );
  }

  function draftRequest(action = 'preview') {
    return {
      action,
      observed: cleanTerms(profile.observed),
      conditions: cleanTerms(profile.conditions),
      pathways: cleanTerms(profile.pathways),
      genes: cleanGenes(profile.genes),
      includePolygenic: profile.includePolygenic,
      includeUpstreamDownstream: profile.includeUpstreamDownstream,
      showMatchesOnly: action === 'apply' ? true : profile.showMatchesOnly,
      ...(action === 'apply' ? { previewFingerprint: preview?.fingerprint } : {}),
    };
  }

  function unresolvedPasteCount() {
    return (pasteResolution?.ambiguous?.length || 0) +
      (pasteResolution?.notRecognized?.length || 0);
  }

  function invalidatePreview(delay = 180, syncGeneList = true) {
    previewRevision += 1;
    profile.activeGeneration = null;
    preview = null;
    previewError = '';
    clearTimeout(previewTimer);
    previewRequest?.abort();
    if (!hasPositiveInput() || !run) {
      previewLoading = false;
      if (syncGeneList) {
        geneListDraft = [];
        geneSections = [];
        pasteText = '';
        pasteResolution = null;
      }
      return;
    }
    previewTimer = setTimeout(
      () => void requestPreview({ allSymbols: syncGeneList, syncGeneList }),
      delay,
    );
  }

  function announceResolutionWhile(current) {
    clearTimeout(resolutionAnnouncementTimer);
    resolutionAnnouncementTimer = setTimeout(() => {
      if (!current()) return;
      message = 'Resolving genes…';
      if (!host().classList.contains('hidden')) render();
    }, 250);
  }

  function clearResolutionAnnouncement() {
    clearTimeout(resolutionAnnouncementTimer);
    resolutionAnnouncementTimer = null;
    if (message === 'Resolving genes…') message = '';
  }

  async function requestPreview({
    allSymbols = false,
    syncGeneList = false,
    renderDuring = true,
  } = {}) {
    if (!run || !hasPositiveInput()) {
      preview = null;
      return null;
    }
    previewRequest?.abort();
    const controller = new AbortController();
    const revision = previewRevision;
    previewRequest = controller;
    previewLoading = true;
    previewError = '';
    announceResolutionWhile(() => revision === previewRevision && previewRequest === controller);
    if (renderDuring) render();
    try {
      const params = new URLSearchParams({
        offset: '0',
        limit: '50',
        q: '',
        presence: 'all',
        ...(allSymbols ? { allSymbols: '1' } : {}),
      });
      const response = await fetch(
        `/api/runs/${encodeURIComponent(run.id)}/genes/preview?${params}`,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-AnnoCat-CSRF': '1',
          },
          body: JSON.stringify(draftRequest('preview')),
          signal: controller.signal,
        },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error || 'Could not resolve genes');
      if (revision !== previewRevision || previewRequest !== controller) return null;
      preview = body;
      if (syncGeneList) {
        const sections = resolveGeneSections(body);
        if (revision !== previewRevision || previewRequest !== controller) return null;
        geneSections = sections;
        const unique = new Map();
        geneSections.flatMap(section => section.genes).forEach(gene => {
          unique.set((gene.symbol || gene.label).toUpperCase(), gene);
        });
        geneListDraft = [...unique.values()];
        pasteText = formatGeneListSections(geneSections);
        pasteResolution = null;
      }
      return body;
    } catch (error) {
      if (error.name !== 'AbortError' && revision === previewRevision && previewRequest === controller) previewError = error.message;
      return null;
    } finally {
      if (previewRequest !== controller) return null;
      clearResolutionAnnouncement();
      previewRequest = null;
      previewLoading = false;
      if (renderDuring && !host().classList.contains('hidden')) {
        render();
        position();
      }
    }
  }

  function resolveGeneSections(overallPreview) {
    if (Array.isArray(overallPreview.geneSections)) {
      return overallPreview.geneSections.map(section => ({
        label: section.label,
        genes: section.genes || [],
        alwaysHeading: true,
      }));
    }
    return overallPreview.allIncludedGenes?.length
      ? [{ label: 'Gene list', genes: overallPreview.allIncludedGenes }]
      : [];
  }

  function updateButton() {
    const button = $('#phenotypes');
    if (!button) return;
    const observed = terms('observed').length;
    const conditions = terms('conditions').length;
    const pathways = terms('pathways').length;
    const genes = terms('genes').length;
    const count = observed + conditions + pathways + genes;
    const label = button.querySelector('span:not([data-phenotype-count])');
    if (label) label.textContent = 'Genes';
    let badge = button.querySelector('[data-phenotype-count]');
    if (!badge) {
      button.insertAdjacentHTML(
        'beforeend',
        '<span class="fui-badge hidden" data-phenotype-count></span>',
      );
      badge = button.querySelector('[data-phenotype-count]');
    }
    badge.textContent = count.toLocaleString();
    badge.classList.toggle('hidden', count === 0);
    button.setAttribute(
      'aria-label',
      `Genes: ${observed} features, ${conditions} conditions, ${pathways} pathways, ${genes} entered genes`,
    );
  }

  function conditionTitle(term) {
    const count = Number(term.subtypeCount);
    if (!Number.isFinite(count)) {
      return 'Includes this condition and its active MONDO subtypes.';
    }
    return `Includes this condition and ${count.toLocaleString()} active MONDO subtype${count === 1 ? '' : 's'}.`;
  }

  function selectedTermsHtml() {
    const items = [
      ...terms('observed').map(term => ({ ...term, kind: 'observed', type: 'Feature' })),
      ...terms('conditions').map(term => ({ ...term, kind: 'conditions', type: 'Condition' })),
      ...terms('pathways').map(term => ({ ...term, kind: 'pathways', type: 'Pathway' })),
    ];
    const genes = terms('genes');
    const geneId = gene => gene.canonicalGeneId || gene.resultGeneId || gene.symbol;
    const enteredGenes = genes.length
      ? `<span ${genes.length === 1 ? `title="Gene · ${escapeHtml(geneId(genes[0]))}"` : ''}><b>${escapeHtml(genes.length === 1 ? genes[0].symbol : `${genes.length.toLocaleString()} entered genes`)}</b>${genes.length === 1 ? `<small>${escapeHtml(geneId(genes[0]))}</small>` : ''}<button type="button" class="fui-button fui-button--small fui-button--icon fui-button--subtle" data-clear-entered-genes aria-label="Remove entered genes">${prototypeIcon('close')}</button></span>`
      : '';
    if (!items.length && !enteredGenes) return '';
    return `<section class="phenotype-selection"><div class="phenotype-chips">${items.map(term => `<span title="${escapeHtml(`${term.type} · ${term.id}${term.kind === 'conditions' ? `. ${conditionTitle(term)}` : ''}`)}"><b>${escapeHtml(term.label)}</b><small>${escapeHtml(term.id)}</small><button type="button" class="fui-button fui-button--small fui-button--icon fui-button--subtle" data-remove-phenotype="${escapeHtml(term.id)}" data-phenotype-kind="${term.kind}" aria-label="Remove ${escapeHtml(term.label)}">${prototypeIcon('close')}</button></span>`).join('')}${enteredGenes}</div></section>`;
  }

  function resolvedPasteGenes() {
    if (!pasteResolution || unresolvedPasteCount()) return [];
    return (pasteResolution.recognized || []).map(item => cleanGenes([item.matches[0]])[0]);
  }

  function currentGeneListDraft() {
    return geneListDraft.length ? geneListDraft : resolvedPasteGenes();
  }

  function geneListHtml() {
    const draft = currentGeneListDraft();
    const selected = savedGeneLists.find(list => list.name === selectedGeneListName);
    const savedOptions = savedGeneLists.map(list => `<option value="${escapeHtml(list.name)}" ${list.name === selectedGeneListName ? 'selected' : ''}>${escapeHtml(list.name)} (${list.genes.length.toLocaleString()})</option>`).join('');
    const unresolved = [
      ...(pasteResolution?.ambiguous || []).map(item => item.entry),
      ...(pasteResolution?.notRecognized || []),
    ];
    return `<section class="gene-paste">
      <textarea class="fui-textarea" rows="8" data-paste-genes aria-label="Gene list" placeholder="Or paste genes here, separated by commas">${escapeHtml(pasteText)}</textarea>
      ${unresolved.length ? `<p class="gene-list-unresolved" role="status"><strong>Unresolved genes:</strong> ${escapeHtml(unresolved.join(', '))}</p>` : ''}
      ${previewError ? `<p class="fui-status-message fui-status-message--error">${escapeHtml(previewError)}</p>` : ''}
      <div class="saved-gene-lists__row">
        <select class="fui-select" data-saved-gene-list aria-label="Saved gene list"><option value="">${savedGeneLists.length ? 'Choose a saved gene list…' : 'No saved gene lists'}</option>${savedOptions}</select>
        <button type="button" class="fui-button" data-use-saved-gene-list ${selected ? '' : 'disabled'}>Use list</button>
        <button type="button" class="fui-button" data-save-gene-list ${draft.length && !unresolved.length && !pasteLoading ? '' : 'disabled'}>Save list</button>
        <button type="button" class="fui-button" data-delete-gene-list ${selected ? '' : 'disabled'}>Delete</button>
      </div>
    </section>`;
  }

  function matchDescription(term) {
    if (!term.matchedText) return '';
    if (term.matchKind === 'externalIdentifier') {
      return `Matched identifier: ${term.matchedText}`;
    }
    if (term.matchKind === 'synonym') {
      const scope = term.synonymScope ? `${term.synonymScope} ` : '';
      return `Matched ${scope}synonym: ${term.matchedText}`;
    }
    return '';
  }

  function resultDisabled(term) {
    return Number(term.geneCount) === 0 &&
      (term.termType === 'condition' || term.termType === 'pathway');
  }

  function firstSelectableResult() {
    return results.findIndex(term => !resultDisabled(term));
  }

  function renderResults() {
    const list = host().querySelector('[data-phenotype-results]');
    if (!list) return;
    const statusRow = text =>
      `<div role="option" aria-selected="false" aria-disabled="true" class="fui-menu-item phenotype-search-state"><span class="fui-menu-item__content"><span class="fui-menu-item__title">${escapeHtml(text)}</span></span></div>`;
    if (searchLoading) {
      list.innerHTML = '';
    } else if (searchComplete && !results.length) {
      list.innerHTML = statusRow(PHENOTYPE_SEARCH_EMPTY);
    } else {
      list.innerHTML = results.map((term, index) => {
        const disabled = resultDisabled(term);
        const count = Number.isInteger(term.geneCount)
          ? `${term.geneCount.toLocaleString()} associated ${term.geneCount === 1 ? 'gene' : 'genes'}`
          : '';
        const description = [
          term.id,
          term.termType === 'condition' ? 'Condition' : term.termType === 'pathway' ? 'Pathway' : term.termType === 'gene' ? 'Gene' : 'Feature',
          count,
          matchDescription(term),
        ].filter(Boolean).join(' · ');
        return `<button id="phenotype-search-option-${index}" type="button" role="option" aria-selected="${!disabled && index === activeIndex}" aria-disabled="${disabled}" class="fui-menu-item ${!disabled && index === activeIndex ? 'active' : ''}" data-phenotype-result="${escapeHtml(term.id)}" ${disabled ? 'disabled' : ''}><span class="fui-menu-item__content"><strong class="fui-menu-item__title">${escapeHtml(term.label)}</strong><small class="fui-menu-item__description">${escapeHtml(description)}</small></span></button>`;
      }).join('');
    }
    const input = host().querySelector('[data-phenotype-search]');
    input?.setAttribute('aria-expanded', String(Boolean(list.innerHTML)));
    const activeResult = results[activeIndex];
    if (input && activeResult && !resultDisabled(activeResult)) {
      input.setAttribute('aria-activedescendant', `phenotype-search-option-${activeIndex}`);
    } else {
      input?.removeAttribute('aria-activedescendant');
    }
    const live = host().querySelector('[data-phenotype-search-status]');
    if (live) {
      live.textContent = searchAnnouncement;
      searchAnnouncement = '';
    }
  }

  function render() {
    const popover = host();
    const contentScrollTop = popover.querySelector('.phenotype-popover__content')?.scrollTop || 0;
    popover.toggleAttribute('aria-busy', applying);
    const hpoReady = Boolean(resources.hpo?.ready);
    const reactomeReady = Boolean(resources.reactome?.ready);
    const hasSelection =
      terms('observed').length ||
      terms('conditions').length ||
      terms('pathways').length ||
      terms('genes').length;
    const validProfile = hasPositiveInput();
    const previewReady = Boolean(preview?.fingerprint) && !previewLoading && !pasteLoading && !unresolvedPasteCount();
    const { canApply, html: scopeSummary } = summarizeGenePreviewScope(preview);
    const enabledSettingCount = activeGeneSettingCount(profile);
    const settingsLabel = enabledSettingCount
      ? `Gene settings, ${enabledSettingCount} enabled`
      : 'Gene settings';
    popover.innerHTML = `<div class="phenotype-popover__content">
        <div class="phenotype-popover__heading"><h2 id="phenotype-popover-title">Add a feature, condition, pathway, or gene</h2><div class="fui-menu phenotype-settings-menu" data-gene-settings><button type="button" class="fui-button fui-button--icon phenotype-settings-menu__trigger ${enabledSettingCount ? 'has-active-settings' : ''}" data-gene-settings-toggle aria-label="${escapeHtml(settingsLabel)}" title="${escapeHtml(settingsLabel)}" aria-controls="phenotype-settings-popover" aria-expanded="${geneSettingsOpen}" aria-haspopup="dialog"><svg class="ui-icon" aria-hidden="true"><use href="#icon-settings"></use></svg><span class="phenotype-settings-menu__indicator" aria-hidden="true"></span></button><div id="phenotype-settings-popover" class="fui-popover fui-popover--menu fui-menu__popover phenotype-settings-menu__popover ${geneSettingsOpen ? '' : 'hidden'}" role="dialog" aria-labelledby="phenotype-settings-title"><strong id="phenotype-settings-title" class="fui-menu-group__label">Gene settings</strong><label class="phenotype-setting-option" title="${escapeHtml(POLYGENIC_ASSOCIATIONS_TOOLTIP)}"><div class="phenotype-setting-option__label">Include polygenic associations for HPO and MONDO</div><input class="fui-switch" type="checkbox" role="switch" data-include-polygenic aria-describedby="phenotype-polygenic-description" ${profile.includePolygenic ? 'checked' : ''}></label><span id="phenotype-polygenic-description" class="phenotype-visually-hidden">${escapeHtml(POLYGENIC_ASSOCIATIONS_TOOLTIP)}</span><label class="phenotype-setting-option" title="${escapeHtml(UPSTREAM_DOWNSTREAM_TOOLTIP)}"><div class="phenotype-setting-option__label">Include upstream/downstream variants (VEP 5 kb)</div><input class="fui-switch" type="checkbox" role="switch" data-include-upstream-downstream aria-describedby="phenotype-upstream-downstream-description" ${profile.includeUpstreamDownstream ? 'checked' : ''}></label><span id="phenotype-upstream-downstream-description" class="phenotype-visually-hidden">${escapeHtml(UPSTREAM_DOWNSTREAM_TOOLTIP)}</span></div></div></div>
        <label class="fui-field phenotype-search-field phenotype-popover__search"><span class="phenotype-visually-hidden">Search names or identifiers</span><input class="fui-input" type="search" data-phenotype-search value="${escapeHtml(searchText)}" autocomplete="off" role="combobox" aria-autocomplete="list" aria-controls="phenotype-search-results" aria-expanded="false" placeholder="Search names or identifiers"><div id="phenotype-search-results" class="phenotype-search-results fui-popover fui-popover--listbox" data-phenotype-results role="listbox"></div></label>
        <span class="phenotype-visually-hidden" data-phenotype-search-status role="status" aria-live="polite" aria-atomic="true"></span>
        ${hpoReady && profile.mondoRelease && reactomeReady
          ? ''
          : `<div class="fui-status-message fui-status-message--warning"><span>${escapeHtml([
            !hpoReady || !profile.mondoRelease ? 'Install phenotype and condition knowledge to add features and conditions.' : '',
            !reactomeReady ? 'Install Reactome to add pathways.' : '',
            'Entered genes remain available.',
          ].filter(Boolean).join(' '))}</span><button type="button" class="fui-button" data-install-hpo>Open Data sources</button></div>`}
        ${selectedTermsHtml()}
        ${geneListHtml()}
        <p class="phenotype-scope-note">${scopeSummary}</p>
        ${message ? `<div class="phenotype-message" role="status"><span>${escapeHtml(message)}</span></div>` : ''}
      </div>
      <footer class="phenotype-popover__footer result-filter-actions"><button type="button" class="fui-button" data-clear-phenotypes ${(hasSelection || profileLoadError) && !applying ? '' : 'disabled'}>Clear</button><button type="button" class="fui-button fui-button--primary" data-apply-phenotypes ${validProfile && previewReady && canApply && !applying ? '' : 'disabled'}>${applying ? 'Applying…' : previewLoading || pasteLoading ? 'Resolving…' : 'Apply'}</button></footer>`;
    popover
      .querySelector('.phenotype-popover__content')
      ?.toggleAttribute('inert', applying);
    const content = popover.querySelector('.phenotype-popover__content');
    if (content) content.scrollTop = contentScrollTop;
    renderResults();
    updateButton();
  }

  function position() {
    const button = $('#phenotypes');
    const popover = host();
    const rect = button.getBoundingClientRect();
    const rootFontSize = Number.parseFloat(
      window.getComputedStyle(document.documentElement).fontSize,
    ) || 16;
    const width = Math.min(51.25 * rootFontSize, window.innerWidth - 24);
    const preferredTop = rect.bottom + 8;
    let top = preferredTop;
    let maxHeight = Math.min(720, window.innerHeight - top - 12);
    if (maxHeight < 320) {
      top = 12;
      maxHeight = Math.min(720, window.innerHeight - 24);
    }
    popover.style.width = `${width}px`;
    popover.style.maxHeight = `${maxHeight}px`;
    popover.style.top = `${top}px`;
    popover.style.left = `${Math.max(12, Math.min(window.innerWidth - width - 12, rect.left + rect.width / 2 - width / 2))}px`;
  }

  async function search(query) {
    request?.abort();
    const controller = new AbortController();
    request = controller;
    searchLoading = true;
    searchComplete = false;
    results = [];
    activeIndex = -1;
    searchAnnouncement = PHENOTYPE_SEARCH_PENDING;
    renderResults();
    try {
      const response = await fetch(
        `/api/phenotypes/terms?q=${encodeURIComponent(query)}&limit=20&runId=${encodeURIComponent(run?.id || '')}&includePolygenic=${profile.includePolygenic}`,
        { signal: controller.signal },
      );
      const body = await response.json();
      if (!response.ok) {
        throw new Error(body.error || 'Could not search genes');
      }
      if (request !== controller) return;
      profile.mondoRelease = body.mondoRelease || null;
      results = body.terms || [];
      searchLoading = false;
      searchComplete = true;
      activeIndex = firstSelectableResult();
      searchAnnouncement = results.length ? '' : PHENOTYPE_SEARCH_EMPTY;
      renderResults();
    } catch (error) {
      if (error.name !== 'AbortError' && request === controller) {
        results = [];
        searchLoading = false;
        searchComplete = false;
        searchAnnouncement = '';
        message = error.message;
        render();
      }
    } finally {
      if (request === controller) request = null;
    }
  }

  function add(term, rerender = true) {
    const kind = term.termType === 'condition' ? 'conditions' : term.termType === 'pathway' ? 'pathways' : term.termType === 'gene' ? 'genes' : 'observed';
    const selected = kind === 'genes' ? cleanGenes([term])[0] : {
        id: term.id,
        label: term.label,
        ...(term.subtypeCount !== undefined
          ? { subtypeCount: term.subtypeCount }
          : {}),
        ...(term.geneCount !== undefined ? { geneCount: term.geneCount } : {}),
      };
    const key = kind === 'genes' ? selected.canonicalGeneId || `SYMBOL:${selected.symbol}` : selected.id;
    if (!terms(kind).some(item => (kind === 'genes' ? item.canonicalGeneId || `SYMBOL:${item.symbol}` : item.id) === key)) {
      profile[kind].push(selected);
    }
    results = [];
    searchText = '';
    searchLoading = false;
    searchComplete = false;
    searchAnnouncement = '';
    activeIndex = -1;
    message = '';
    invalidatePreview();
    if (rerender) {
      render();
      queueMicrotask(() => host().querySelector('[data-phenotype-search]')?.focus());
    }
    return true;
  }

  function makeManualGeneList(genes) {
    profile.observed = [];
    profile.conditions = [];
    profile.pathways = [];
    profile.genes = cleanGenes(genes);
    profile.includePolygenic = false;
    profile.showMatchesOnly = true;
  }

  async function resolvePaste({ restoreFocus = false } = {}) {
    const revision = pasteRevision;
    const input = host().querySelector('[data-paste-genes]');
    const selectionStart = input?.selectionStart ?? pasteText.length;
    const selectionEnd = input?.selectionEnd ?? pasteText.length;
    const entries = splitGeneListEntries(pasteText);
    if (!entries.length) {
      pasteRequest?.abort();
      pasteRequest = null;
      pasteLoading = false;
      clearResolutionAnnouncement();
      pasteResolution = null;
      geneListDraft = [];
      geneSections = [];
      makeManualGeneList([]);
      preview = null;
      previewLoading = false;
      message = '';
      render();
      if (restoreFocus) {
        host().querySelector('[data-paste-genes]')?.focus({ preventScroll: true });
      }
      return;
    }
    pasteRequest?.abort();
    const controller = new AbortController();
    pasteRequest = controller;
    pasteLoading = true;
    message = '';
    announceResolutionWhile(() => revision === pasteRevision && pasteRequest === controller);
    try {
      const response = await fetch('/api/phenotypes/terms', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-AnnoCat-CSRF': '1' },
        body: JSON.stringify({ entries, runId: run.id }),
        signal: controller.signal,
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error || 'Could not review the pasted list');
      if (revision !== pasteRevision) return;
      pasteResolution = body;
      geneListDraft = (body.recognized || []).map(item => cleanGenes([item.matches[0]])[0]);
      geneSections = geneListDraft.length
        ? [{ label: 'Gene list', genes: geneListDraft }]
        : [];
      makeManualGeneList(geneListDraft);
      preview = null;
      previewLoading = false;
      if (geneListDraft.length) {
        await requestPreview({ renderDuring: false });
      }
    } catch (error) {
      if (error.name !== 'AbortError') message = error.message;
    } finally {
      if (pasteRequest !== controller || revision !== pasteRevision) return;
      clearResolutionAnnouncement();
      pasteRequest = null;
      pasteLoading = false;
      render();
      position();
      if (restoreFocus) {
        const nextInput = host().querySelector('[data-paste-genes]');
        nextInput?.focus({ preventScroll: true });
        nextInput?.setSelectionRange(selectionStart, selectionEnd);
      }
    }
  }

  async function loadGeneLists() {
    const response = await fetch('/api/gene-lists');
    const body = await response.json();
    if (!response.ok) throw new Error(body.error || 'Could not load saved gene lists');
    savedGeneLists = Array.isArray(body.lists) ? body.lists : [];
    if (!savedGeneLists.some(list => list.name === selectedGeneListName)) {
      selectedGeneListName = '';
    }
  }

  function useGeneList(genes, name = '') {
    if (!genes.length) return;
    profile.includePolygenic = false;
    geneListDraft = genes.map(savedGeneValue);
    geneSections = [{ label: name || 'Gene list', genes: geneListDraft }];
    pasteText = formatGeneListSections(geneSections);
    pasteResolution = null;
    message = `${genes.length.toLocaleString()} genes added${name ? ` from ${name}` : ''}.`;
    void resolvePaste();
    render();
  }

  async function saveGeneList(name) {
    const genes = currentGeneListDraft();
    if (!name?.trim() || !genes.length || unresolvedPasteCount()) return;
    const response = await fetch('/api/gene-lists', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-AnnoCat-CSRF': '1' },
      body: JSON.stringify({ action: 'save', name, genes: genes.map(savedGeneValue) }),
    });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error || 'Could not save the gene list');
    savedGeneLists = body.lists || [];
    selectedGeneListName = savedGeneLists.find(list =>
      list.name.toLowerCase() === name.trim().toLowerCase())?.name || '';
    message = `${name.trim()} saved.`;
    render();
  }

  async function deleteGeneList() {
    if (!selectedGeneListName) return;
    const response = await fetch('/api/gene-lists', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-AnnoCat-CSRF': '1' },
      body: JSON.stringify({ action: 'delete', name: selectedGeneListName }),
    });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error || 'Could not delete the gene list');
    savedGeneLists = body.lists || [];
    selectedGeneListName = '';
    message = 'Gene list deleted.';
    render();
  }

  async function apply({ closePopover = true } = {}) {
    applying = true;
    applyStartedAt = Date.now();
    const updateElapsed = () => {
      const elapsed = Math.max(0, Math.floor((Date.now() - applyStartedAt) / 1000));
      message = `Updating gene matches · ${elapsed} s`;
      render();
      position();
    };
    updateElapsed();
    applyElapsedTimer = window.setInterval(updateElapsed, 1000);
    try {
      const response = await fetch(
        `/api/runs/${encodeURIComponent(run.id)}/phenotypes`,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-AnnoCat-CSRF': '1',
          },
          body: JSON.stringify(draftRequest('apply')),
        },
      );
      const body = await response.json();
      if (!response.ok) {
        throw new Error(body.error || 'Could not apply the gene profile');
      }
      profile = normalizeProfile(body);
      updateButton();
      if (closePopover) close();
      await onApply?.(profile, 'apply');
    } catch (error) {
      message = error.message;
    } finally {
      window.clearInterval(applyElapsedTimer);
      applyElapsedTimer = null;
      applying = false;
      if (!host().classList.contains('hidden')) {
        render();
        position();
      }
    }
  }

  async function clear({ closePopover = true } = {}) {
    const response = await fetch(
      `/api/runs/${encodeURIComponent(run.id)}/phenotypes`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-AnnoCat-CSRF': '1' },
        body: JSON.stringify({ action: 'clear' }),
      },
    );
    const body = await response.json();
    if (!response.ok) {
      message = body.error || 'Could not clear the phenotype profile';
      render();
      return;
    }
    profile = normalizeProfile(body);
    profileLoadError = '';
    message = '';
    preview = null;
    previewError = '';
    previewLoading = false;
    previewRevision += 1;
    geneListDraft = [];
    geneSections = [];
    pasteText = '';
    pasteResolution = null;
    updateButton();
    if (closePopover) close();
    await onApply?.(profile, 'clear');
    if (!closePopover && !host().classList.contains('hidden')) {
      render();
      position();
    }
  }

  async function applyRemoval() {
    clearTimeout(previewTimer);
    previewRequest?.abort();
    profile.activeGeneration = null;
    preview = null;
    previewError = '';
    previewLoading = false;
    previewRevision += 1;
    if (!hasPositiveInput()) {
      await clear({ closePopover: false });
      return;
    }
    render();
    const nextPreview = await requestPreview({ allSymbols: true, syncGeneList: true });
    if (nextPreview) await apply({ closePopover: false });
  }

  function handleClick(event) {
    const settingsToggle = event.target.closest('[data-gene-settings-toggle]');
    if (settingsToggle) {
      setGeneSettingsOpen(!geneSettingsOpen);
      return;
    }
    if (geneSettingsOpen && !event.target.closest('[data-gene-settings]')) {
      setGeneSettingsOpen(false);
    }
    const result = event.target.closest('[data-phenotype-result]');
    if (result) {
      const term = results.find(
        item => item.id === result.dataset.phenotypeResult,
      );
      if (term && !resultDisabled(term)) add(term);
      return;
    }
    const remove = event.target.closest('[data-remove-phenotype]');
    if (remove) {
      const removeFromAppliedProfile = Boolean(profile.activeGeneration);
      profile[remove.dataset.phenotypeKind] = terms(
        remove.dataset.phenotypeKind,
      ).filter(item => item.id !== remove.dataset.removePhenotype);
      message = '';
      if (removeFromAppliedProfile) void applyRemoval();
      else {
        invalidatePreview();
        render();
      }
      return;
    }
    if (event.target.closest('[data-clear-entered-genes]')) {
      const removeFromAppliedProfile = Boolean(profile.activeGeneration);
      profile.genes = [];
      message = '';
      if (removeFromAppliedProfile) void applyRemoval();
      else {
        invalidatePreview();
        render();
      }
      return;
    }
    if (event.target.closest('[data-save-gene-list]')) {
      const sectionName = geneSections.length === 1 ? geneSections[0].label : '';
      const suggestedName = selectedGeneListName ||
        (!['Gene list', 'Entered genes'].includes(sectionName) ? sectionName : '');
      void requestFluentText({
        title: 'Save gene list',
        label: 'List name',
        value: suggestedName,
      }).then(name => name ? saveGeneList(name) : null).catch(error => {
        message = error.message;
        render();
      });
      return;
    }
    if (event.target.closest('[data-use-saved-gene-list]')) {
      const list = savedGeneLists.find(item => item.name === selectedGeneListName);
      if (list) useGeneList(list.genes, list.name);
      return;
    }
    if (event.target.closest('[data-delete-gene-list]')) {
      void deleteGeneList().catch(error => {
        message = error.message;
        render();
      });
      return;
    }
    if (event.target.closest('[data-view-missing-genes]')) {
      openMissingGenes();
      return;
    }
    if (event.target.closest('[data-apply-phenotypes]')) void apply();
    if (event.target.closest('[data-clear-phenotypes]')) void clear();
    if (event.target.closest('[data-install-hpo]')) {
      close();
      showPage('resources');
    }
  }

  function handleInput(event) {
    if (event.target.matches('[data-include-polygenic]')) {
      profile.includePolygenic = event.target.checked;
      clearTimeout(timer);
      request?.abort();
      request = null;
      results = [];
      searchLoading = false;
      searchComplete = false;
      searchAnnouncement = '';
      activeIndex = -1;
      invalidatePreview(0, true);
      render();
      position();
      const query = searchText.trim();
      if (query.length >= 2) void search(query);
      queueMicrotask(() => host().querySelector('[data-include-polygenic]')?.focus());
      return;
    }
    if (event.target.matches('[data-include-upstream-downstream]')) {
      profile.includeUpstreamDownstream = event.target.checked;
      invalidatePreview(0, false);
      render();
      position();
      queueMicrotask(() => host().querySelector('[data-include-upstream-downstream]')?.focus());
      return;
    }
    if (event.target.matches('[data-saved-gene-list]')) {
      selectedGeneListName = event.target.value;
      render();
      return;
    }
    if (event.target.matches('[data-paste-genes]')) {
      pasteText = event.target.value;
      profile.includePolygenic = false;
      const polygenic = host().querySelector('[data-include-polygenic]');
      if (polygenic) polygenic.checked = false;
      pasteRevision += 1;
      pasteRequest?.abort();
      pasteRequest = null;
      pasteLoading = false;
      clearResolutionAnnouncement();
      previewRequest?.abort();
      previewRequest = null;
      previewLoading = false;
      clearTimeout(previewTimer);
      pasteResolution = null;
      geneListDraft = [];
      geneSections = [];
      clearTimeout(pasteTimer);
      pasteTimer = setTimeout(() => {
        pasteTimer = null;
        void resolvePaste({ restoreFocus: true });
      }, 900);
      host().querySelector('[data-save-gene-list]')?.setAttribute('disabled', '');
      host().querySelector('[data-apply-phenotypes]')?.setAttribute('disabled', '');
      return;
    }
    if (!event.target.matches('[data-phenotype-search]')) return;
    clearTimeout(timer);
    request?.abort();
    request = null;
    searchText = event.target.value;
    searchLoading = false;
    searchComplete = false;
    searchAnnouncement = '';
    results = [];
    activeIndex = -1;
    renderResults();
    const query = searchText.trim();
    if (query.length < 2) {
      return;
    }
    timer = setTimeout(() => search(query), 220);
  }

  function handleKeydown(event) {
    if (event.key === 'Escape' && geneSettingsOpen) {
      event.preventDefault();
      event.stopPropagation();
      setGeneSettingsOpen(false, true);
      return;
    }
    if (event.key === 'Escape' && !results.length) {
      event.preventDefault();
      close(true);
      return;
    }
    if (!event.target.matches('[data-phenotype-search]') || !results.length) {
      return;
    }
    if (!['ArrowDown', 'ArrowUp', 'Enter', 'Escape'].includes(event.key)) {
      return;
    }
    event.preventDefault();
    if (event.key === 'Escape') {
      results = [];
      searchComplete = false;
      activeIndex = -1;
      renderResults();
      return;
    }
    const selectable = results
      .map((term, index) => resultDisabled(term) ? -1 : index)
      .filter(index => index >= 0);
    if (!selectable.length) return;
    const current = selectable.indexOf(activeIndex);
    if (event.key === 'ArrowDown') {
      activeIndex = selectable[Math.min(selectable.length - 1, current + 1)];
    }
    if (event.key === 'ArrowUp') {
      activeIndex = selectable[Math.max(0, current < 0 ? 0 : current - 1)];
    }
    if (event.key === 'Enter') {
      const term = results[activeIndex];
      if (term && !resultDisabled(term)) add(term);
      return;
    }
    renderResults();
  }

  function close(returnFocus = false) {
    clearTimeout(timer);
    clearTimeout(previewTimer);
    clearTimeout(pasteTimer);
    request?.abort();
    previewRequest?.abort();
    pasteRequest?.abort();
    request = null;
    pasteTimer = null;
    pasteRequest = null;
    geneSettingsOpen = false;
    $('#phenotype-popover')?.classList.add('hidden');
    $('#phenotypes')?.setAttribute('aria-expanded', 'false');
    if (returnFocus) $('#phenotypes')?.focus();
  }

  async function sync(currentRun, currentResources = resources) {
    const runChanged = run?.id !== currentRun?.id;
    if (runChanged) resetDraftState();
    run = currentRun;
    resources = currentResources;
    if (!run) {
      profile = emptyProfile();
      updateButton();
      return profile;
    }
    const response = await fetch(
      `/api/runs/${encodeURIComponent(run.id)}/phenotypes`,
    );
    const body = await response.json();
    if (!response.ok) {
      profile = emptyProfile();
      profileLoadError = body.error || 'Could not load the phenotype profile';
      updateButton();
      throw new Error(profileLoadError);
    }
    profile = normalizeProfile(body);
    profileLoadError = '';
    updateButton();
    return profile;
  }

  async function open(currentRun, currentResources) {
    const popover = host();
    if (!popover.classList.contains('hidden')) {
      close();
      return;
    }
    const alreadySynced = run?.id === currentRun?.id;
    if (!alreadySynced) resetDraftState();
    run = currentRun;
    resources = currentResources;
    message = '';
    results = [];
    searchText = '';
    searchLoading = false;
    searchComplete = false;
    searchAnnouncement = '';
    geneSettingsOpen = false;
    activeIndex = -1;
    popover.classList.remove('hidden');
    $('#phenotypes')?.setAttribute('aria-expanded', 'true');
    position();
    render();
    try {
      let syncError = profileLoadError ? new Error(profileLoadError) : null;
      if (!alreadySynced) {
        try {
          await sync(run, resources);
          syncError = null;
        } catch (error) {
          syncError = error;
        }
      }
      try {
        await loadGeneLists();
      } catch (error) {
        message = syncError
          ? `${syncError.message} Saved gene lists could not be loaded: ${error.message}`
          : error.message;
      }
      if (syncError && !message) message = syncError.message;
      render();
      position();
      if (!syncError && pasteText.trim() && !pasteResolution && !geneListDraft.length) {
        await resolvePaste();
      } else if (!syncError && hasPositiveInput() && !preview) {
        await requestPreview({ allSymbols: true, syncGeneList: true });
      }
      queueMicrotask(() =>
        host().querySelector('[data-phenotype-search]')?.focus(),
      );
    } catch (error) {
      profile = emptyProfile();
      message = error.message;
      render();
    }
  }

  function setGeneSettingsOpen(open, returnFocus = false) {
    geneSettingsOpen = open;
    const trigger = host().querySelector('[data-gene-settings-toggle]');
    trigger?.setAttribute('aria-expanded', String(open));
    host().querySelector('[data-gene-settings] .phenotype-settings-menu__popover')
      ?.classList.toggle('hidden', !open);
    if (returnFocus) trigger?.focus();
  }

  document.addEventListener('click', event => {
    const popover = $('#phenotype-popover');
    const path = event.composedPath();
    if (
      popover &&
      !popover.classList.contains('hidden') &&
      !path.includes(popover) &&
      !path.includes($('#phenotypes')) &&
      !path.some(node => node?.matches?.('dialog[open]'))
    ) {
      close();
    }
  });
  window.addEventListener('resize', () => {
    if (!host().classList.contains('hidden')) position();
  });

  function hasActiveProfile() {
    return Boolean(profile?.activeGeneration);
  }

  updateButton();
  return { open, close, sync, hasActiveProfile };
}
