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
    ? matches.map(match => [
      match.selectedItem,
      match.itemType,
      match.geneSymbol,
      match.relation,
    ].filter(Boolean).join(' · ')).join('\n')
    : [dependencyValue('matchedSelectedItems'), dependencyValue('matchedItemTypes')]
      .filter(Boolean).join(' · ');
  return {
    display: value && value !== 'Not reported' ? String(value) : 'No match',
    tooltip: tooltip || 'No selected item matches this variant gene.',
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
    }))
    .filter(section => section.genes.length);
  if (populated.length <= 1) {
    return populated[0]?.genes.map(gene => gene.label).join(', ') || '';
  }
  return populated.map(section =>
    `[${section.label}]\n${section.genes.map(gene => gene.label).join(', ')}`,
  ).join('\n\n');
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
  let message = '';
  let applying = false;
  let applyStartedAt = 0;
  let applyElapsedTimer = null;
  let preview = null;
  let previewLoading = false;
  let previewError = '';
  let previewTimer = null;
  let previewRequest = null;
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

  function emptyProfile() {
    return {
      observed: [],
      excluded: [],
      conditions: [],
      pathways: [],
      genes: [],
      excludedGenes: [],
      combination: 'any',
      showMatchesOnly: false,
      addAsAbsent: false,
      limitToLinkedGenes: false,
      mondoRelease: null,
      monarchSuggestions: null,
      monarchError: null,
    };
  }

  function normalizeProfile(value = {}) {
    return {
      ...value,
      observed: Array.isArray(value.observed) ? value.observed : [],
      excluded: Array.isArray(value.excluded) ? value.excluded : [],
      conditions: Array.isArray(value.conditions) ? value.conditions : [],
      pathways: Array.isArray(value.pathways) ? value.pathways : [],
      genes: Array.isArray(value.genes) ? value.genes : [],
      excludedGenes: Array.isArray(value.excludedGenes) ? value.excludedGenes : [],
      combination: value.combination === 'every' ? 'every' : 'any',
      showMatchesOnly: Boolean(value.showMatchesOnly),
      addAsAbsent: false,
      limitToLinkedGenes: Boolean(value.limitToLinkedGenes),
      monarchSuggestions: value.monarchSuggestions || null,
      monarchError: value.monarchError || null,
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

  function terms(kind) {
    return Array.isArray(profile?.[kind]) ? profile[kind] : [];
  }

  function cleanTerms(items) {
    return items.map(({ id, label }) => ({ id, label }));
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
      excluded: cleanTerms(profile.excluded),
      conditions: cleanTerms(profile.conditions),
      pathways: cleanTerms(profile.pathways),
      genes: cleanTerms(profile.genes),
      excludedGenes: profile.excludedGenes,
      combination: profile.combination,
      showMatchesOnly: action === 'apply' ? true : profile.showMatchesOnly,
      limitToLinkedGenes: false,
      requestMonarchSuggestions: false,
      ...(action === 'apply' ? { previewFingerprint: preview?.fingerprint } : {}),
    };
  }

  function unresolvedPasteCount() {
    return (pasteResolution?.ambiguous?.length || 0) +
      (pasteResolution?.notRecognized?.length || 0);
  }

  function invalidatePreview(delay = 180, syncGeneList = true) {
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
    previewRequest = controller;
    previewLoading = true;
    previewError = '';
    if (renderDuring) render();
    try {
      const params = new URLSearchParams({
        offset: '0',
        limit: '50',
        q: '',
        presence: 'all',
        ...(allSymbols && (!syncGeneList || geneSourceSections().length <= 1)
          ? { allSymbols: '1' }
          : {}),
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
      preview = body;
      if (syncGeneList) {
        geneSections = await resolveGeneSections(body, controller.signal);
        const unique = new Map();
        geneSections.flatMap(section => section.genes).forEach(gene => {
          unique.set(gene.label.toUpperCase(), gene);
        });
        geneListDraft = [...unique.values()];
        pasteText = formatGeneListSections(geneSections);
        pasteResolution = null;
      }
      return body;
    } catch (error) {
      if (error.name !== 'AbortError') previewError = error.message;
      return null;
    } finally {
      if (previewRequest !== controller) return null;
      previewRequest = null;
      previewLoading = false;
      if (renderDuring && !host().classList.contains('hidden')) {
        render();
        position();
      }
    }
  }

  function geneSourceSections() {
    return [
      ...terms('observed').map(term => ({ kind: 'observed', label: term.label, terms: [term] })),
      ...terms('conditions').map(term => ({ kind: 'conditions', label: term.label, terms: [term] })),
      ...terms('pathways').map(term => ({ kind: 'pathways', label: term.label, terms: [term] })),
      ...(terms('genes').length ? [{
        kind: 'genes',
        label: terms('genes').length === 1 ? terms('genes')[0].label : 'Entered genes',
        terms: terms('genes'),
      }] : []),
    ];
  }

  async function resolveGeneSections(overallPreview, signal) {
    const sections = geneSourceSections();
    if (sections.length <= 1) {
      return sections.length ? [{
        label: sections[0].label,
        genes: overallPreview.allIncludedGenes || [],
      }] : [];
    }
    return Promise.all(sections.map(async section => {
      const requestBody = {
        ...draftRequest('preview'),
        observed: [],
        excluded: [],
        conditions: [],
        pathways: [],
        genes: [],
        excludedGenes: [],
        [section.kind]: cleanTerms(section.terms),
      };
      const response = await fetch(
        `/api/runs/${encodeURIComponent(run.id)}/genes/preview?offset=0&limit=1&q=&presence=all&allSymbols=1`,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-AnnoCat-CSRF': '1',
          },
          body: JSON.stringify(requestBody),
          signal,
        },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error || `Could not resolve ${section.label}`);
      return { label: section.label, genes: body.allIncludedGenes || [] };
    }));
  }

  function updateButton() {
    const button = $('#phenotypes');
    if (!button) return;
    const observed = terms('observed').length;
    const excluded = terms('excluded').length;
    const conditions = terms('conditions').length;
    const pathways = terms('pathways').length;
    const genes = terms('genes').length;
    const count = observed + excluded + conditions + pathways + genes;
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
      `Genes: ${observed} observed features, ${excluded} explicitly absent features, ${conditions} conditions, ${pathways} pathways, ${genes} entered genes`,
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
      ...terms('excluded').map(term => ({ ...term, kind: 'excluded', type: 'Absent feature' })),
    ];
    const genes = terms('genes');
    const enteredGenes = genes.length
      ? `<span><b>${escapeHtml(genes.length === 1 ? genes[0].label : `${genes.length.toLocaleString()} entered genes`)}</b><small>${genes.length === 1 ? `Gene · ${escapeHtml(genes[0].id)}` : 'Gene list'}</small><button type="button" class="fui-button fui-button--icon fui-button--subtle" data-clear-entered-genes aria-label="Remove entered genes">${prototypeIcon('close')}</button></span>`
      : '';
    return `<section class="phenotype-selection"><h3>Selected</h3>${
      items.length || enteredGenes
        ? `<div class="phenotype-chips">${items.map(term => `<span ${term.kind === 'conditions' ? `title="${escapeHtml(conditionTitle(term))}"` : ''}><b>${escapeHtml(term.label)}</b><small>${escapeHtml(term.type)} · ${escapeHtml(term.id)}</small><button type="button" class="fui-button fui-button--icon fui-button--subtle" data-remove-phenotype="${escapeHtml(term.id)}" data-phenotype-kind="${term.kind}" aria-label="Remove ${escapeHtml(term.label)}">${prototypeIcon('close')}</button></span>`).join('')}${enteredGenes}</div>`
        : '<p class="phenotype-empty">No items selected</p>'
    }</section>`;
  }

  function resolvedPasteGenes() {
    if (!pasteResolution || unresolvedPasteCount()) return [];
    return (pasteResolution.recognized || []).map(item => ({
      id: item.matches[0].id,
      label: item.matches[0].label,
    }));
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
    const count = preview
      ? `${preview.includedGenes.toLocaleString()} ${preview.includedGenes === 1 ? 'gene' : 'genes'} · ${preview.genesInResult.toLocaleString()} in this result`
      : pasteLoading || previewLoading ? 'Resolving genes…' : '';
    return `<section class="gene-paste">
      <label class="fui-field"><span class="fui-field__label">Gene list${count ? `<small>${escapeHtml(count)}</small>` : ''}</span><textarea class="fui-textarea" rows="5" data-paste-genes placeholder="BRCA1, BRCA2, ENSG00000141510">${escapeHtml(pasteText)}</textarea></label>
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

  function renderResults() {
    const list = host().querySelector('[data-phenotype-results]');
    if (!list) return;
    list.innerHTML = results
      .map(
        (term, index) =>
          `<button type="button" role="option" aria-selected="${index === activeIndex}" class="fui-menu-item ${index === activeIndex ? 'active' : ''}" data-phenotype-result="${escapeHtml(term.id)}"><span class="fui-menu-item__content"><strong class="fui-menu-item__title">${escapeHtml(term.label)}</strong><small class="fui-menu-item__description">${escapeHtml([term.id, term.termType === 'condition' ? 'Condition' : term.termType === 'pathway' ? 'Pathway' : term.termType === 'gene' ? 'Gene' : 'Feature', Number.isInteger(term.geneCount) ? `${term.geneCount.toLocaleString()} genes` : '', matchDescription(term)].filter(Boolean).join(' · '))}</small></span></button>`,
      )
      .join('');
  }

  function render() {
    const popover = host();
    const contentScrollTop = popover.querySelector('.phenotype-popover__content')?.scrollTop || 0;
    popover.toggleAttribute('aria-busy', applying);
    const hpoReady = Boolean(resources.hpo?.ready);
    const reactomeReady = Boolean(resources.reactome?.ready);
    const hasSelection =
      terms('observed').length ||
      terms('excluded').length ||
      terms('conditions').length ||
      terms('pathways').length ||
      terms('genes').length;
    const validProfile = hasPositiveInput();
    const previewReady = Boolean(preview?.fingerprint) && !previewLoading && !pasteLoading && !unresolvedPasteCount();
    const selectedCount = terms('observed').length + terms('excluded').length +
      terms('conditions').length + terms('pathways').length + terms('genes').length;
    const scopeSummary = preview
      ? `${selectedCount.toLocaleString()} selected · ${preview.totalGenes.toLocaleString()} ${preview.totalGenes === 1 ? 'gene' : 'genes'} · ${preview.genesInResult.toLocaleString()} in this result. `
      : '';
    popover.innerHTML = `<header class="phenotype-popover__header"><h2 id="phenotype-popover-title" class="fui-section-heading">Genes</h2></header>
      <div class="phenotype-popover__content">
        <label class="fui-field phenotype-search-field phenotype-popover__search"><span class="fui-field__label">Add a feature, condition, pathway, or gene</span><input class="fui-input" type="search" data-phenotype-search autocomplete="off" role="combobox" aria-autocomplete="list" aria-controls="phenotype-search-results" aria-expanded="false" placeholder="Search names or identifiers"><div id="phenotype-search-results" class="phenotype-search-results fui-popover fui-popover--listbox" data-phenotype-results role="listbox"></div></label>
        ${hpoReady && profile.mondoRelease && reactomeReady
          ? ''
          : `<div class="fui-status-message fui-status-message--warning"><span>${escapeHtml([
            !hpoReady || !profile.mondoRelease ? 'Install HPO and MONDO to add features and conditions.' : '',
            !reactomeReady ? 'Install Reactome to add pathways.' : '',
            'Entered genes remain available.',
          ].filter(Boolean).join(' '))}</span><button type="button" class="fui-button" data-install-hpo>Open Data sources</button></div>`}
        ${selectedTermsHtml()}
        ${geneListHtml()}
        <p class="phenotype-scope-note">${scopeSummary}Shows variants in genes linked to the selected items. It does not rank variants.</p>
        ${message ? `<div class="phenotype-message" role="status"><span>${escapeHtml(message)}</span></div>` : ''}
      </div>
      <footer class="phenotype-popover__footer result-filter-actions"><button type="button" class="fui-button" data-clear-phenotypes ${hasSelection && !applying ? '' : 'disabled'}>Clear</button><button type="button" class="fui-button fui-button--primary" data-apply-phenotypes ${validProfile && previewReady && !applying ? '' : 'disabled'}>${applying ? 'Applying…' : previewLoading || pasteLoading ? 'Resolving…' : 'Apply'}</button></footer>`;
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
    const width = Math.min(560, window.innerWidth - 24);
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
    request = new AbortController();
    try {
      const response = await fetch(
        `/api/phenotypes/terms?q=${encodeURIComponent(query)}&limit=20&runId=${encodeURIComponent(run?.id || '')}`,
        { signal: request.signal },
      );
      const body = await response.json();
      if (!response.ok) {
        throw new Error(body.error || 'Could not search genes');
      }
      profile.mondoRelease = body.mondoRelease || null;
      results = body.terms || [];
      activeIndex = results.length ? 0 : -1;
      renderResults();
      const input = host().querySelector('[data-phenotype-search]');
      input?.setAttribute('aria-expanded', String(results.length > 0));
    } catch (error) {
      if (error.name !== 'AbortError') {
        results = [];
        message = error.message;
        render();
      }
    }
  }

  function add(term, rerender = true) {
    const kind = term.termType === 'condition' ? 'conditions' : term.termType === 'pathway' ? 'pathways' : term.termType === 'gene' ? 'genes' : profile.addAsAbsent ? 'excluded' : 'observed';
    if (kind === 'observed') profile.excluded = terms('excluded').filter(item => item.id !== term.id);
    if (kind === 'excluded') profile.observed = terms('observed').filter(item => item.id !== term.id);
    if (!terms(kind).some(item => item.id === term.id)) {
      profile[kind].push({
        id: term.id,
        label: term.label,
        ...(term.subtypeCount !== undefined
          ? { subtypeCount: term.subtypeCount }
          : {}),
        ...(term.geneCount !== undefined ? { geneCount: term.geneCount } : {}),
      });
    }
    results = [];
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
    profile.excluded = [];
    profile.conditions = [];
    profile.pathways = [];
    profile.genes = cleanTerms(genes);
    profile.excludedGenes = [];
    profile.combination = 'any';
    profile.showMatchesOnly = true;
  }

  async function resolvePaste({ restoreFocus = false } = {}) {
    const revision = pasteRevision;
    const input = host().querySelector('[data-paste-genes]');
    const selectionStart = input?.selectionStart ?? pasteText.length;
    const selectionEnd = input?.selectionEnd ?? pasteText.length;
    const entries = splitGeneListEntries(pasteText);
    if (!entries.length) {
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
      geneListDraft = (body.recognized || []).map(item => ({
        id: item.matches[0].id,
        label: item.matches[0].label,
      }));
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
    makeManualGeneList(genes);
    geneListDraft = cleanTerms(genes);
    geneSections = [{ label: name || 'Gene list', genes: geneListDraft }];
    pasteText = formatGeneListSections(geneSections);
    pasteResolution = null;
    message = `${genes.length.toLocaleString()} genes added${name ? ` from ${name}` : ''}.`;
    invalidatePreview(0, false);
    render();
  }

  async function saveGeneList(name) {
    const genes = currentGeneListDraft();
    if (!name?.trim() || !genes.length || unresolvedPasteCount()) return;
    const response = await fetch('/api/gene-lists', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-AnnoCat-CSRF': '1' },
      body: JSON.stringify({ action: 'save', name, genes }),
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

  async function apply() {
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
      close();
      await onApply?.(profile, 'apply');
    } catch (error) {
      message = error.message;
    } finally {
      window.clearInterval(applyElapsedTimer);
      applyElapsedTimer = null;
      applying = false;
      if (!host().classList.contains('hidden')) render();
    }
  }

  async function clear() {
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
    preview = null;
    previewError = '';
    previewLoading = false;
    updateButton();
    close();
    await onApply?.(profile, 'clear');
  }

  function handleClick(event) {
    const result = event.target.closest('[data-phenotype-result]');
    if (result) {
      const term = results.find(
        item => item.id === result.dataset.phenotypeResult,
      );
      if (term) add(term);
      return;
    }
    const remove = event.target.closest('[data-remove-phenotype]');
    if (remove) {
      profile[remove.dataset.phenotypeKind] = terms(
        remove.dataset.phenotypeKind,
      ).filter(item => item.id !== remove.dataset.removePhenotype);
      message = '';
      invalidatePreview();
      render();
      return;
    }
    if (event.target.closest('[data-clear-entered-genes]')) {
      profile.genes = [];
      profile.excludedGenes = [];
      message = '';
      invalidatePreview();
      render();
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
    if (event.target.closest('[data-apply-phenotypes]')) void apply();
    if (event.target.closest('[data-clear-phenotypes]')) void clear();
    if (event.target.closest('[data-install-hpo]')) {
      close();
      showPage('resources');
    }
  }

  function handleInput(event) {
    if (event.target.matches('[data-saved-gene-list]')) {
      selectedGeneListName = event.target.value;
      render();
      return;
    }
    if (event.target.matches('[data-paste-genes]')) {
      pasteText = event.target.value;
      pasteRevision += 1;
      pasteRequest?.abort();
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
    const query = event.target.value.trim();
    if (query.length < 2) {
      results = [];
      renderResults();
      return;
    }
    timer = setTimeout(() => search(query), 220);
  }

  function handleKeydown(event) {
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
      renderResults();
      return;
    }
    if (event.key === 'ArrowDown') {
      activeIndex = Math.min(results.length - 1, activeIndex + 1);
    }
    if (event.key === 'ArrowUp') {
      activeIndex = Math.max(0, activeIndex - 1);
    }
    if (event.key === 'Enter') {
      add(results[Math.max(0, activeIndex)]);
      return;
    }
    renderResults();
  }

  function close(returnFocus = false) {
    clearTimeout(previewTimer);
    clearTimeout(pasteTimer);
    previewRequest?.abort();
    pasteRequest?.abort();
    pasteTimer = null;
    pasteRequest = null;
    host().classList.add('hidden');
    $('#phenotypes')?.setAttribute('aria-expanded', 'false');
    if (returnFocus) $('#phenotypes')?.focus();
  }

  async function sync(currentRun, currentResources = resources) {
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
      updateButton();
      throw new Error(body.error || 'Could not load the phenotype profile');
    }
    profile = normalizeProfile(body);
    updateButton();
    return profile;
  }

  async function open(currentRun, currentResources) {
    const popover = host();
    if (!popover.classList.contains('hidden')) {
      close();
      return;
    }
    run = currentRun;
    resources = currentResources;
    message = '';
    results = [];
    activeIndex = -1;
    preview = null;
    previewError = '';
    pasteText = '';
    pasteResolution = null;
    geneListDraft = [];
    geneSections = [];
    pasteRevision += 1;
    popover.classList.remove('hidden');
    $('#phenotypes')?.setAttribute('aria-expanded', 'true');
    position();
    render();
    try {
      await sync(run, resources);
      try {
        await loadGeneLists();
      } catch (error) {
        message = error.message;
      }
      render();
      position();
      if (hasPositiveInput()) {
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
