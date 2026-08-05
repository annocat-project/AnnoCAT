export const PROFILE_EVIDENCE_DEPENDENCIES = [
  'selectedConditionMatches',
  'matchedSelectedConditions',
  'selectedConditionRelation',
  'directFeatureMatches',
  'absentFeatureConflict',
  'phenotypeEvidenceDetails',
];

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

export function matchedGeneCountText(value) {
  if (!Number.isInteger(value) || value < 0) return '';
  return `${value.toLocaleString()} ${value === 1 ? 'gene' : 'genes'} found. `;
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

  function emptyProfile() {
    return {
      observed: [],
      excluded: [],
      conditions: [],
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
    return popover;
  }

  function terms(kind) {
    return Array.isArray(profile?.[kind]) ? profile[kind] : [];
  }

  function cleanTerms(items) {
    return items.map(({ id, label }) => ({ id, label }));
  }

  function updateButton() {
    const button = $('#phenotypes');
    if (!button) return;
    const observed = terms('observed').length;
    const excluded = terms('excluded').length;
    const conditions = terms('conditions').length;
    const count = observed + excluded + conditions;
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
      `Phenotypes: ${observed} observed, ${excluded} explicitly absent, ${conditions} known conditions`,
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
      ...terms('excluded').map(term => ({ ...term, kind: 'excluded', type: 'Absent feature' })),
    ];
    return `<section class="phenotype-selection"><h3>Selected</h3>${
      items.length
        ? `<div class="phenotype-chips">${items.map(term => `<span ${term.kind === 'conditions' ? `title="${escapeHtml(conditionTitle(term))}"` : ''}><b>${escapeHtml(term.label)}</b><small>${escapeHtml(term.type)} · ${escapeHtml(term.id)}</small><button type="button" class="fui-button fui-button--icon fui-button--subtle" data-remove-phenotype="${escapeHtml(term.id)}" data-phenotype-kind="${term.kind}" aria-label="Remove ${escapeHtml(term.label)}">${prototypeIcon('close')}</button></span>`).join('')}</div>`
        : '<p class="phenotype-empty">No phenotypes selected</p>'
    }</section>`;
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
          `<button type="button" role="option" aria-selected="${index === activeIndex}" class="fui-menu-item ${index === activeIndex ? 'active' : ''}" data-phenotype-result="${escapeHtml(term.id)}"><span class="fui-menu-item__content"><strong class="fui-menu-item__title">${escapeHtml(term.label)}</strong><small class="fui-menu-item__description">${escapeHtml([term.id, term.termType === 'condition' ? 'Condition' : 'Feature', matchDescription(term)].filter(Boolean).join(' · '))}</small></span></button>`,
      )
      .join('');
  }

  function render() {
    const popover = host();
    popover.toggleAttribute('aria-busy', applying);
    const hpoReady = Boolean(resources.hpo?.ready);
    if (!hpoReady) {
      popover.innerHTML = `<header class="phenotype-popover__header"><h2 id="phenotype-popover-title" class="fui-section-heading">Phenotypes</h2></header><div class="phenotype-unavailable"><p>Install phenotype and condition knowledge to compare selected features and conditions with genes.</p>
        ${
          terms('observed').length || terms('excluded').length || terms('conditions').length
            ? selectedTermsHtml()
            : ''
        }
        <button type="button" class="fui-button fui-button--primary" data-install-hpo>Open Data sources</button></div>`;
      updateButton();
      return;
    }
    const hasSelection =
      terms('observed').length ||
      terms('excluded').length ||
      terms('conditions').length;
    const validProfile =
      terms('observed').length || terms('conditions').length;
    const needsMatches = validProfile && !profile.activeGeneration;
    const matchedGeneSummary = matchedGeneCountText(
      profile.activeGeneration?.matchedGeneCount,
    );
    popover.innerHTML = `<header class="phenotype-popover__header"><h2 id="phenotype-popover-title" class="fui-section-heading">Phenotypes</h2></header>
      <label class="fui-field phenotype-search-field phenotype-popover__search"><span class="fui-field__label">Add a feature or condition</span><input class="fui-input" type="search" data-phenotype-search autocomplete="off" role="combobox" aria-autocomplete="list" aria-controls="phenotype-search-results" aria-expanded="false" placeholder="Search features, conditions, HPO or MONDO IDs"><div id="phenotype-search-results" class="phenotype-search-results fui-popover fui-popover--listbox" data-phenotype-results role="listbox"></div></label>
      <div class="phenotype-popover__content">
        ${
          profile.mondoRelease
            ? ''
            : '<div class="fui-status-message fui-status-message--warning"><span>Update phenotype and condition knowledge to add known conditions.</span><button type="button" class="fui-button" data-install-hpo>Open Data sources</button></div>'
        }
        ${selectedTermsHtml()}
        ${needsMatches ? '<p class="fui-status-message fui-status-message--warning">Select Apply to update phenotype matches.</p>' : ''}
        <p class="phenotype-scope-note">${matchedGeneSummary}Shows phenotype similarity and selected-condition links for genes in this result. It does not rank variants or estimate causality.</p>
        ${message ? `<div class="phenotype-message" role="status"><span>${escapeHtml(message)}</span></div>` : ''}
      </div>
      <footer class="phenotype-popover__footer result-filter-actions"><button type="button" class="fui-button" data-clear-phenotypes ${hasSelection && !applying ? '' : 'disabled'}>Clear</button><button type="button" class="fui-button fui-button--primary" data-apply-phenotypes ${validProfile && !applying ? '' : 'disabled'}>${applying ? 'Applying…' : 'Apply'}</button></footer>`;
    popover
      .querySelector('.phenotype-popover__content')
      ?.toggleAttribute('inert', applying);
    renderResults();
    updateButton();
  }

  function position() {
    const button = $('#phenotypes');
    const popover = host();
    const rect = button.getBoundingClientRect();
    const width = Math.min(480, window.innerWidth - 24);
    popover.style.width = `${width}px`;
    popover.style.top = `${rect.bottom + 8}px`;
    popover.style.left = `${Math.max(12, Math.min(window.innerWidth - width - 12, rect.left + rect.width / 2 - width / 2))}px`;
  }

  async function search(query) {
    request?.abort();
    request = new AbortController();
    try {
      const response = await fetch(
        `/api/phenotypes/terms?q=${encodeURIComponent(query)}&limit=20`,
        { signal: request.signal },
      );
      const body = await response.json();
      if (!response.ok) {
        throw new Error(body.error || 'Could not search phenotype and condition terms');
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

  function add(term) {
    const kind = term.termType === 'condition' ? 'conditions' : 'observed';
    if (kind === 'observed') profile.excluded = terms('excluded').filter(item => item.id !== term.id);
    if (!terms(kind).some(item => item.id === term.id)) {
      profile[kind].push({
        id: term.id,
        label: term.label,
        ...(term.subtypeCount !== undefined
          ? { subtypeCount: term.subtypeCount }
          : {}),
      });
    }
    results = [];
    activeIndex = -1;
    message = '';
    render();
    queueMicrotask(() => host().querySelector('[data-phenotype-search]')?.focus());
    return true;
  }

  async function apply() {
    applying = true;
    applyStartedAt = Date.now();
    const updateElapsed = () => {
      const elapsed = Math.max(0, Math.floor((Date.now() - applyStartedAt) / 1000));
      message = `Matching selected features and conditions · ${elapsed} s`;
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
          body: JSON.stringify({
            action: 'apply',
            observed: cleanTerms(profile.observed),
            excluded: cleanTerms(profile.excluded),
            conditions: cleanTerms(profile.conditions),
            limitToLinkedGenes: false,
            requestMonarchSuggestions: false,
          }),
        },
      );
      const body = await response.json();
      if (!response.ok) {
        throw new Error(body.error || 'Could not apply the phenotype profile');
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
      render();
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
    popover.classList.remove('hidden');
    $('#phenotypes')?.setAttribute('aria-expanded', 'true');
    position();
    render();
    try {
      await sync(run, resources);
      render();
      position();
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
      !path.includes($('#phenotypes'))
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

  return { open, close, sync, hasActiveProfile };
}
