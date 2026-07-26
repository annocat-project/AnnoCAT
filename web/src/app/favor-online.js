const FIELD_DETAILS = {
  rsid: ['dbSNP identifier', 'dbSNP rs identifier returned by FAVOR when one is available.'],
  vcf: ['Variant coordinate', 'GRCh38 chromosome-position-reference-alternate coordinate returned by FAVOR.'],
  gene: ['Gene symbol', 'Gene symbol associated with this variant by FAVOR.'],
  consequence: ['Consequence', 'Functional consequence category returned by FAVOR.'],
  clinicalsignificance: ['Clinical significance', 'Clinical significance category returned by FAVOR. Verify clinical assertions against the primary ClinVar record.'],
  caddphred: ['CADD PHRED score', 'Rank-scaled CADD deleteriousness score; higher values indicate stronger predicted functional impact.'],
  revel: ['REVEL score', 'Missense ensemble score from 0 to 1; higher values indicate greater predicted pathogenicity.'],
  alphamissense: ['AlphaMissense score', 'Missense pathogenicity score returned by AlphaMissense.'],
  spliceaidsmax: ['SpliceAI maximum delta score', 'Maximum SpliceAI delta score across acceptor and donor gain and loss predictions.'],
  siftcat: ['SIFT prediction', 'SIFT missense-effect category returned by FAVOR.'],
  polyphencat: ['PolyPhen-2 prediction', 'PolyPhen-2 missense-effect category returned by FAVOR.'],
  metasvmpred: ['MetaSVM prediction', 'MetaSVM ensemble missense-effect category returned by FAVOR.'],
  gnomadaf: ['gnomAD allele frequency', 'Overall allele frequency from the gnomAD data represented by FAVOR.'],
  bravoaf: ['TOPMed BRAVO allele frequency', 'Overall allele frequency from TOPMed BRAVO represented by FAVOR.'],
  tgall: ['1000 Genomes allele frequency', 'Overall allele frequency from the 1000 Genomes data represented by FAVOR.'],
  apcconservation: ['Conservation aPC', 'FAVOR annotation principal-component score summarizing conservation features.'],
  apcepigenetics: ['Epigenetics aPC', 'FAVOR annotation principal-component score summarizing epigenetic features.'],
  apcproteinfunction: ['Protein function aPC', 'FAVOR annotation principal-component score summarizing protein-function features.']
};
const FAVOR_CONFIRMATION_STORAGE_KEY = 'annocat.favorTransmissionConfirmed.v1';

function readableFieldName(value) {
  return String(value || 'FAVOR field')
    .replace(/_/g, ' ')
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/\b[a-z]/g, letter => letter.toUpperCase());
}

export function favorFieldPresentation(path, leaf) {
  const field = String(leaf || path || '').replace(/[^a-z0-9]/gi, '').toLowerCase();
  return FIELD_DETAILS[field] || [
    readableFieldName(leaf || path),
    'Fixed FAVOR annotation stored with this report.'
  ];
}

export function createFavorOnline({
  $,
  escapeHtml,
  prototypeIcon,
  openFluentDialog,
  togglePopover,
  getState,
  collectFilteredAlleles,
  collectSelectedAlleles,
  refreshResultSchema,
  setResultStatus,
  showNotice,
  onServiceChange
}) {
  let service = {
    enabled: true,
    maxVariants: 10000,
    name: 'FAVOR',
    purpose: 'Online GRCh38 annotation for selected or filtered result sets.'
  };
  let runStatus = null;
  let activeRunId = null;
  let busy = false;
  let feedback = '';

  function serviceCardHtml() {
    const state = service.enabled ? 'Enabled' : 'Disabled';
    const action = service.enabled ? 'Disable' : 'Enable';
    const appearance = service.enabled ? '' : 'fui-button--primary';
    return `<article class="fui-card fui-card--content-compact online-service-card" data-service-card="favor">
      <div class="source-card-copy">
        <h2 class="fui-card__title fui-card__title-row">
          <span>FAVOR</span>
          <small class="fui-badge">Online service</small>
          <span class="source-state fui-badge">${state}</span>
        </h2>
        <p class="source-card-description fui-card__description">${escapeHtml(service.purpose || 'Online GRCh38 annotation for selected or filtered result sets.')}</p>
        <p class="source-card-storage fui-card__metadata">Fixed standard field set &middot; Up to ${Number(service.maxVariants || 10000).toLocaleString()} variants per operation</p>
      </div>
      <div class="source-card-meta">
        <div class="source-actions">
          <button type="button" class="fui-button ${appearance}" data-favor-service-toggle>${prototypeIcon(service.enabled ? 'close' : 'check')}<span>${action}</span></button>
        </div>
      </div>
    </article>`;
  }

  function renderServiceCard() {
    const host = $('#source-list');
    if (!host) return;
    host.querySelector('[data-service-card="favor"]')?.remove();
    host.insertAdjacentHTML('beforeend', serviceCardHtml());
    host.querySelector('[data-favor-service-toggle]')?.addEventListener('click', toggleService);
  }

  function selectionCounts() {
    const state = getState();
    return {
      selected: Number(state.selectionCount || 0),
      current: Number(state.resultTotal || 0)
    };
  }

  function reportCoverage(status) {
    const total = Number(status?.totalCached ?? status?.cached ?? 0);
    const found = Number(status?.found || 0);
    const outcomes = [];
    if (status?.notFound) outcomes.push(`${Number(status.notFound).toLocaleString()} not found`);
    if (status?.ambiguous) outcomes.push(`${Number(status.ambiguous).toLocaleString()} ambiguous`);
    if (status?.errors) outcomes.push(`${Number(status.errors).toLocaleString()} errors`);
    const coverage = total
      ? `${found.toLocaleString()} of ${total.toLocaleString()} variants annotated`
      : `${found.toLocaleString()} variants annotated`;
    return `${coverage}${outcomes.length ? ` · ${outcomes.join(' · ')}` : ''}`;
  }

  function statusCopy() {
    if (!activeRunId) return 'Open a completed GRCh38 report to add FAVOR annotations.';
    if (!service.enabled) return 'Enable FAVOR in Data sources to use online enrichment.';
    if (!runStatus?.hasData) return 'This report does not contain FAVOR annotations yet.';
    return reportCoverage(runStatus);
  }

  function renderPopover() {
    const panel = $('#favor-popover');
    if (!panel) return;
    panel.classList.add('fui-popover--compact', 'fui-popover--anchor-center');
    const counts = selectionCounts();
    const maximum = Number(service.maxVariants || runStatus?.maxVariants || 10000);
    const currentTooLarge = counts.current > maximum;
    const selectedTooLarge = counts.selected > maximum;
    const selectedUnavailable = busy || !service.enabled || !activeRunId || counts.selected === 0 || selectedTooLarge;
    const selectedTitle = counts.selected === 0
      ? 'Select variants in the table to enable this action'
      : selectedTooLarge
        ? `Select no more than ${maximum.toLocaleString()} variants`
        : 'Get online annotations for selected variants';
    panel.innerHTML = `<div class="fui-popover__header fui-popover__header--borderless favor-popover__header">
        <div>
          <h2 class="fui-subtitle">Online annotations</h2>
          <p class="fui-text--secondary">Add clinical evidence, population frequencies, prediction scores, and conservation summaries. Up to ${maximum.toLocaleString()} variants per request.</p>
        </div>
        <button type="button" class="fui-button fui-button--icon favor-popover__help" data-favor-summary aria-label="About online annotation fields" title="About online annotation fields"><span aria-hidden="true">?</span></button>
      </div>
      <div class="fui-popover__content favor-popover__content">
        <p class="favor-popover__status">${escapeHtml(feedback || statusCopy())}</p>
        ${currentTooLarge ? `<p class="fui-status-message fui-status-message--info">Current results exceed ${maximum.toLocaleString()} variants. Narrow the table with search or filters before enrichment.</p>` : ''}
        <div class="favor-popover__actions">
          <button type="button" class="fui-button" data-favor-enrich="selected" title="${escapeHtml(selectedTitle)}" ${selectedUnavailable ? 'disabled' : ''}>${prototypeIcon('check')}<span>Get selected${counts.selected ? ` (${counts.selected.toLocaleString()})` : ''}</span></button>
          <button type="button" class="fui-button fui-button--primary" data-favor-enrich="current" ${busy || !service.enabled || !activeRunId || counts.current === 0 || currentTooLarge ? 'disabled' : ''}>${prototypeIcon('download')}<span>${busy ? 'Getting annotations...' : `Get current results${counts.current ? ` (${counts.current.toLocaleString()})` : ''}`}</span></button>
        </div>
      </div>`;
    panel.querySelectorAll('[data-favor-enrich]').forEach(button => {
      button.addEventListener('click', () => enrich(button.dataset.favorEnrich));
    });
    panel.querySelector('[data-favor-summary]')?.addEventListener('click', event => {
      event.stopPropagation();
      togglePopover();
      openFluentDialog(ensureSummaryDialog());
    });
  }

  function ensureSummaryDialog() {
    let dialog = $('#favor-summary-dialog');
    if (dialog) return dialog;
    document.body.insertAdjacentHTML('beforeend', `<dialog id="favor-summary-dialog" class="favor-summary-dialog fui-dialog" tabindex="-1" aria-labelledby="favor-summary-title">
      <form method="dialog" class="fui-dialog__surface">
        <header class="fui-dialog__header">
          <div>
            <p class="kicker">Online annotations</p>
            <h2 id="favor-summary-title">FAVOR summary fields</h2>
            <p class="fui-dialog__description">AnnoCAT stores these 18 fields from FAVOR's summary API response.</p>
          </div>
          <button type="submit" value="close" class="fui-button fui-button--icon" aria-label="Close"><svg class="ui-icon" aria-hidden="true"><use href="#icon-close"></use></svg></button>
        </header>
        <div class="fui-dialog__content fui-dialog__content--scrollable">
          <dl class="favor-summary-fields">
            <div><dt>Variant context</dt><dd>dbSNP identifier, variant coordinate, gene symbol, and consequence</dd></div>
            <div><dt>Clinical evidence</dt><dd>Clinical significance</dd></div>
            <div><dt>Prediction scores</dt><dd>CADD PHRED, REVEL, AlphaMissense, SpliceAI maximum delta, SIFT, PolyPhen-2, and MetaSVM</dd></div>
            <div><dt>Population frequencies</dt><dd>gnomAD, TOPMed BRAVO, and 1000 Genomes allele frequencies</dd></div>
            <div><dt>Annotation-PC summaries</dt><dd>Conservation, epigenetics, and protein-function aPC scores</dd></div>
          </dl>
          <p class="fui-text--secondary favor-summary-note">FAVOR's full catalog contains additional fields that are not returned by this summary request.</p>
          <a class="fui-link" href="https://favor-beta.genohub.org/docs/data" target="_blank" rel="noopener noreferrer">Explore FAVOR's full annotation catalog</a>
        </div>
        <footer class="fui-dialog__footer"><div class="fui-dialog__actions"><button type="submit" value="close" class="fui-button">Close</button></div></footer>
      </form>
    </dialog>`);
    return $('#favor-summary-dialog');
  }

  function ensureConsentDialog() {
    let dialog = $('#favor-consent-dialog');
    if (dialog) return dialog;
    document.body.insertAdjacentHTML('beforeend', `<dialog id="favor-consent-dialog" class="favor-consent-dialog fui-dialog" tabindex="-1" aria-labelledby="favor-consent-title">
      <form method="dialog" class="fui-dialog__surface">
        <header class="fui-dialog__header"><div><p class="kicker">Online enrichment</p><h2 id="favor-consent-title">Continue with FAVOR enrichment?</h2></div></header>
        <div class="fui-dialog__content"><p class="fui-dialog__description" data-favor-consent-copy></p><label class="fui-checkbox-field favor-consent-dialog__preference"><input class="fui-checkbox" type="checkbox" data-favor-consent-remember><span>Don't show this again</span></label></div>
        <footer class="fui-dialog__footer"><div class="fui-dialog__actions"><button type="submit" value="cancel" class="fui-button">Cancel</button><button type="submit" value="confirm" class="fui-button fui-button--primary">Continue</button></div></footer>
      </form>
    </dialog>`);
    return $('#favor-consent-dialog');
  }

  function confirmEnrichment(count) {
    if (localStorage.getItem(FAVOR_CONFIRMATION_STORAGE_KEY) === 'true') return Promise.resolve(true);
    const dialog = ensureConsentDialog();
    dialog.querySelector('[data-favor-consent-copy]').textContent =
      `AnnoCAT will send chromosome, position, reference allele, and alternate allele for ${count.toLocaleString()} GRCh38 variants to FAVOR. Genotypes, sample identifiers, notes, phenotypes, filenames, and VCF contents are not included.`;
    dialog.querySelector('[data-favor-consent-remember]').checked = false;
    dialog.returnValue = '';
    return new Promise(resolve => {
      dialog.addEventListener('close', () => {
        const confirmed = dialog.returnValue === 'confirm';
        if (confirmed && dialog.querySelector('[data-favor-consent-remember]').checked) {
          localStorage.setItem(FAVOR_CONFIRMATION_STORAGE_KEY, 'true');
        }
        resolve(confirmed);
      }, { once: true });
      openFluentDialog(dialog);
    });
  }

  async function enrich(scope) {
    if (busy || !activeRunId || !service.enabled) return;
    const counts = selectionCounts();
    const count = scope === 'selected' ? counts.selected : counts.current;
    const maximum = Number(service.maxVariants || 10000);
    if (!count || count > maximum) return;
    if (!await confirmEnrichment(count)) return;
    busy = true;
    feedback = `Preparing ${count.toLocaleString()} variants...`;
    setResultStatus(`Preparing ${count.toLocaleString()} variants for online annotations...`, { busy: true });
    renderPopover();
    try {
      const alleleIds = scope === 'selected'
        ? await collectSelectedAlleles()
        : await collectFilteredAlleles();
      if (alleleIds.length !== count) throw new Error(`Expected ${count.toLocaleString()} variants but loaded ${alleleIds.length.toLocaleString()}.`);
      feedback = `Adding FAVOR annotations for ${count.toLocaleString()} variants...`;
      setResultStatus(`Getting online annotations for ${count.toLocaleString()} variants...`, { busy: true });
      renderPopover();
      const response = await fetch(`/api/runs/${encodeURIComponent(activeRunId)}/favor/enrich`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-AnnoCat-CSRF': '1'
        },
        body: JSON.stringify({ alleleIds, consent: true })
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || 'FAVOR enrichment could not be completed');
      const requested = Number(result.requested || count);
      const completed = reportCoverage({ ...result, totalCached: result.totalCached ?? requested });
      await updateForRun(getState().currentResultRun);
      try {
      await refreshResultSchema({
        sourceId: 'favor-online',
        preferredFields: ['clinicalSignificance', 'gnomadAf', 'caddPhred', 'revel', 'spliceaiDsMax']
      });
        feedback = completed;
      } catch (error) {
        feedback = `${completed} Reload the report to display the new fields (${error.message}).`;
      }
      const resultParts = [`${Number(result.found || 0).toLocaleString()} found`];
      if (result.notFound) resultParts.push(`${Number(result.notFound).toLocaleString()} not found`);
      if (result.ambiguous) resultParts.push(`${Number(result.ambiguous).toLocaleString()} ambiguous`);
      if (result.errors) resultParts.push(`${Number(result.errors).toLocaleString()} errors`);
      setResultStatus(`Online annotations: ${resultParts.join(' / ')}`, { tone: 'success' });
    } catch (error) {
      const failure = `FAVOR enrichment could not be completed: ${error.message}`;
      await updateForRun(getState().currentResultRun).catch(() => {});
      feedback = failure;
      setResultStatus('Online annotation failed', { tone: 'error' });
    } finally {
      busy = false;
      renderPopover();
    }
  }

  async function toggleService(event) {
    const button = event.currentTarget;
    button.disabled = true;
    try {
      const response = await fetch('/api/services/favor', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-AnnoCat-CSRF': '1'
        },
        body: JSON.stringify({ enabled: !service.enabled })
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || 'FAVOR service setting could not be saved');
      service = { ...service, ...result };
      feedback = '';
      renderServiceCard();
      updateControls();
      onServiceChange?.(service.enabled);
    } catch (error) {
      showNotice(error.message);
      button.disabled = false;
    }
  }

  function updateControls() {
    const button = $('#favor');
    if (button) {
      const available = Boolean(activeRunId && service.enabled);
      button.disabled = !available;
      button.title = !activeRunId
        ? 'Open a completed report to use FAVOR'
        : service.enabled
          ? 'Add FAVOR annotations to selected or filtered variants'
          : 'Enable FAVOR in Data sources';
    }
    renderPopover();
  }

  async function updateForRun(run) {
    activeRunId = run?.id || null;
    runStatus = null;
    feedback = '';
    if (activeRunId) {
      const response = await fetch(`/api/runs/${encodeURIComponent(activeRunId)}/favor`);
      const result = await response.json();
      if (response.ok) runStatus = result;
    }
    updateControls();
  }

  async function initialize() {
    const button = $('#favor');
    const phenotypesButton = $('#phenotypes');
    if (button && phenotypesButton) {
      button.parentElement?.insertBefore(button, phenotypesButton);
    }
    button?.addEventListener('click', event => {
      event.stopPropagation();
      renderPopover();
      togglePopover();
    });
    const response = await fetch('/api/services/favor');
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || 'FAVOR service configuration is unavailable');
    service = { ...service, ...result };
    renderServiceCard();
    updateControls();
    onServiceChange?.(service.enabled);
  }

  return {
    initialize,
    updateControls,
    updateForRun,
    isEnabled: () => Boolean(service.enabled),
    resetConfirmation: () => localStorage.removeItem(FAVOR_CONFIRMATION_STORAGE_KEY)
  };
}
