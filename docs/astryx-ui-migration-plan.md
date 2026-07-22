# AnnoCAT Astryx UI Migration Plan

Status: Proposed  
Scope: Preserve the working AnnoCAT application while migrating presentation to React and Astryx  
Primary rule: No production screen is removed until its replacement passes the same real-data acceptance tests

## 1. Objective

Adopt Astryx for AnnoCAT's application shell and user-interface components without replacing or duplicating the working Rust, DuckDB, Parquet, report, resource, task, filtering, selection, candidate, export, or security implementations.

The migration is developed as one complete parallel React application. Results is the first parity checkpoint because it has the densest interaction contract, but the production application is not cut over page by page. The existing interface remains usable until every baseline page and cross-page flow passes, then the complete React application replaces it in one production cutover.

The migration is complete only when:

- real reports, including large WGS results, behave at least as well as the existing viewer;
- every production feature has either moved to the new UI or is deliberately routed to the existing screen;
- the old implementation for a migrated screen has been deleted;
- no user needs Node.js, Rust, or a development server to run a release;
- installed data sources, settings, results, imported reports, candidates, and notes survive the update unchanged.

## 2. Scope boundaries

### Included in baseline migration

- Application shell, responsive navigation, status surface, and coherent iconography.
- Results library entry and the complete Results workspace.
- Real result paging, searching, filtering, sorting, columns, selection, candidates, exports, details, rename, notes, sharing, and importing.
- Existing dynamic field catalogs and unknown future evidence fields.
- Existing local-first behavior and native file/save dialogs.
- Production asset compilation and embedding in the Rust release.
- Removal of replaced HTML, JavaScript, and CSS at the final production cutover.

### Deferred until baseline parity passes

- New phenotype/HPO search and ranking.
- Gene Context and exon-map views.
- GenCC/ClinGen presentation.
- New inheritance-compatible presets.
- Portable review-state schema changes.
- New gnomAD and ClinVar retained-field contracts.
- Family/pedigree analysis, automated ACMG classification, or diagnostic claims.

These items remain governed by `annocat-clinical-variant-review-improvement-plan-v3.docx`. They must not delay or obscure the baseline migration.

## 3. Non-negotiable architectural rules

1. **The Rust service remains the source of truth.** React displays and edits state through the existing APIs. It does not implement an independent WGS query engine.
2. **DuckDB remains responsible for complete-result queries.** Browser-side filtering and sorting are allowed only for a bounded page or an explicitly proven small-result fast path.
3. **The report field catalog remains dynamic.** The UI must not rely on a fixed list of database columns.
4. **One owner per feature.** Candidate membership, selection semantics, filter compilation, exports, report packaging, and task state must not acquire competing implementations.
5. **Temporary duplication is time-bounded.** During a screen migration the old and new renderers may coexist behind a development route, but each migration gate includes deletion of the replaced old renderer.
6. **Do not change canonical report or cache formats for visual migration.** Data-contract changes belong to separate clinical-plan work packages.
7. **No Node.js runtime dependency in releases.** Node and Vite are build-time development tools only; release assets are static and served by the existing Rust application.
8. **Preserve local data locations.** UI updates must not move or recreate resources, downloads, results, settings, or run metadata.
9. **Preserve security boundaries.** Imported text remains escaped, queries remain parameterized, links remain allowlisted, ZIP validation remains in Rust, and mutations retain CSRF protection.
10. **Prefer deletion over wrappers.** Add a module only when it owns a cohesive contract or replaces duplicated logic. Do not create a layer that merely renames another layer.
11. **Use stable, pinned frontend dependencies.** Pin Astryx `0.1.7` stable and one exact Lucide version in the lockfile. Canary examples may inform design, but the release must not depend on canary packages unless a separately reviewed component is genuinely unavailable in stable.
12. **Use one presentation vocabulary.** Source names, task states, dates, decimal storage units (`KB`, `MB`, `GB`) and rates (`KB/s`, `MB/s`, `GB/s`) follow `ui-task-coherence-implementation-plan.md` and one shared formatter rather than page-specific prose.
13. **Keep release assets offline.** Fonts, icons, scripts, styles and component assets are bundled; production viewing must not require a CDN or remote runtime resource.

## 4. Target architecture

### Rust service

Retain ownership of:

- report discovery, import, rename, notes, sharing, and native pickers;
- DuckDB/Parquet paging, counts, filtering, sorting, and filtered exports;
- result detail responses, transcript consequences, evidence, and provenance;
- candidate persistence and bounded batch updates;
- annotation, resource, installation, download, and task state;
- file-system paths, security validation, checksums, and atomic writes.

The migration may make small API additions only where the existing UI behavior cannot be represented cleanly. An API change must be independently useful and covered by a Rust test; it must not exist only to accommodate an Astryx component.

### React application boundary

Use one route-aware application shell and one typed HTTP client. Every production
surface has an explicit owner from the beginning, even though the surfaces are
migrated incrementally. Suggested ownership:

```text
src/
  app/
    App.tsx                 one shell, route outlet and first-run boundary
    routes.ts               typed hash routes and URL parsing
    navigation.ts           labels, Lucide icons and Tasks summary only
  api/
    client.ts              shared fetch, CSRF, errors, abort handling
    results.ts             reports, pages, counts, details, exports
    review.ts              candidates, rename, notes
    annotation.ts          input, profiles, readiness, start and recovery
    resources.ts           catalog, profiles, configuration and removal
    tasks.ts               task snapshots and runtime actions
    settings.ts            paths, preferences and native picker actions
    application.ts         about, first-run readiness and shared app metadata
  annotation/
    AnnotationPage.tsx
  browse/
    BrowseResultsPage.tsx
  results/
    ResultsWorkspace.tsx
    ResultToolbar.tsx
    VariantTable.tsx
    ColumnSelector.tsx
    FilterDialog.tsx
    VariantInspector.tsx
    selection.ts           client representation of existing semantics
    rowWindow.ts           bounded visible-row window over server pages
  resources/
    DataSourcesPage.tsx
    ProfileInstallDialog.tsx
  tasks/
    TasksPage.tsx
  settings/
    SettingsPage.tsx
  about/
    AboutDialog.tsx
  first-run/
    FirstRunDialog.tsx
  components/
    AppIcon.tsx             Lucide-only icon mapping rendered through Astryx
    AsyncState.tsx          shared loading/error/retry presentation
```

This is a responsibility map, not a requirement to create every file immediately. Start with the smallest structure that keeps API state out of visual components.

### Page and flow ownership

| Route or surface | React owner | Server authority | Legacy bridge allowed until |
|---|---|---|---|
| `#/annotate` — New Annotation | `annotation/AnnotationPage` | annotation input, readiness, recovery and start APIs | Phase 9E passes |
| `#/browse` — completed/imported reports | `browse/BrowseResultsPage` | run discovery and native import picker | Phase 9A passes |
| `#/results/:runId` — report viewer | `results/ResultsWorkspace` | DuckDB queries, details, review state and exports | Phase 8 passes |
| `#/sources` — Data Sources | `resources/DataSourcesPage` | source catalog, profiles, configuration, install intent and removal | Phase 9C passes |
| `#/tasks` — Tasks | `tasks/TasksPage` | one task snapshot and runtime task actions | Phase 9B passes |
| `#/settings` — Settings | `settings/SettingsPage` | paths, preferences and native directory pickers | Phase 9D passes |
| About dialog | `about/AboutDialog` | version and application metadata | Phase 9F passes |
| First-run dialog | `first-run/FirstRunDialog` | readiness and setup preference | Phase 9F passes |
| Shared-report import | launched by Browse or First-run; completion returns to `#/results/:runId` | native picker, validation and atomic import | Phase 9A passes |
| Global navigation and task badge | `app/App` and `app/navigation` | route state plus the same task summary used by Tasks | Phase 9G passes |

There is no generic page-level store that mirrors all server state. Each route owns
its transient UI state and consumes typed API modules. A small application-session
state may retain an unfinished annotation draft, return target, Browse scroll
position and first-run visibility while the SPA remains open; it must not copy
server-owned resources, reports, candidates or tasks and must not put sensitive VCF
paths into `localStorage`. A source row, annotation readiness notice and Tasks row
may present the same server state, but only Tasks exposes runtime progress controls.

Use hash routes (`#/annotate`, `#/browse`, `#/results/<run-id>`, `#/sources`,
`#/tasks`, and `#/settings`) through a small typed parser. This avoids adding a
router dependency or requiring Rust history-fallback rules. The registry exists
before page migration so browser refresh and deep links have one contract. An
unmigrated route deliberately renders the existing page through the development
bridge; it must never render an empty placeholder or silently redirect to Results.

### Astryx responsibility

Astryx owns presentation and local interaction primitives:

- `AppShell`, `SideNav`, `Layout`, `LayoutPanel`, and `ResizeHandle`;
- `Table` presentation and supported table interaction hooks;
- `PowerSearch`, selectors, dialogs, menus, overflow controls, buttons, tokens, metadata lists, collapsibles, skeletons, banners, and toasts;
- responsive structure, focusable component primitives, and theme tokens.

Astryx does not own query semantics, evidence interpretation, candidate identity, report persistence, or clinical rules.

### Astryx template adoption strategy

Use Astryx templates as reviewed composition references, not as application
architecture or feature bundles. Scaffold a template into a temporary development
location when its source is useful, copy only the smallest relevant composition,
and delete the scaffold after the AnnoCAT component is implemented. Never copy a
template's mock records, local query state, pagination, selection model, task model,
or demo-specific components into production.

The following installed templates are the best fits:

| AnnoCAT surface | Astryx template reference | Adopt | Deliberately retain from AnnoCAT |
|---|---|---|---|
| Application shell | `shell-side-nav` | Full-height `AppShell`, collapsible/resizable navigation and responsive content frame | Hash routes, first-run logic, task summary, return targets and all page state |
| Browse Results | `table-page` | Dense page header, compact action toolbar and row-oriented library | Native import picker, report discovery, newest-first ordering, reopen behavior and validation |
| Results workspace | `table-grouped` | Table/inspector split, `ResizeHandle`, dense PowerSearch/toolbar composition | DuckDB paging, bounded row window, dynamic fields, multi-sort, column persistence, selection exclusions, candidates, exports and detail contracts |
| Tasks | `incident-console` plus `ProgressBarWithValueLabel` and `ProgressBarIndeterminate` blocks | Frame-first grouped rows, status control, compact inspector/detail treatment and one progress bar per operation | The existing task snapshot, queue/concurrency semantics, phases, recovery, cancellation and history. The WIP incident domain components are not copied |
| Data Sources | `library` for the profile/category header and `settings-sidebar` for compact configurable rows | Browsable profile summary, compact inventory groups and inline configuration hierarchy | Catalog identity, source readiness, retained fields, dependency order, installation intent, update/removal rules and **View task** linkage |
| Settings | `settings-sidebar` | Nav-switched or single-scroll settings sections and compact editable rows | Native directory pickers, real stored preferences and backend validation |
| New Annotation | `form-two-column` only for responsive review layout; Astryx form blocks for the steps | Responsive form grid, review summary and fixed action hierarchy | Input inspection, local/VCF-review modes, readiness, recovery, one annotation request and return-to-Tasks flow |
| Variant Details | Inspector frame from `table-grouped`; metadata composition from `detail-page` only where compact | Resizable inspector, section rhythm, metadata alignment and responsive full-screen fallback | AnnoCAT's evidence order, variant/transcript scope, provenance, candidates, clinical meaning and dynamic source fields |
| First run | `DialogFullscreenDialog` structure only when the viewport is narrow | Accessible modal framing and action layout | The two existing choices and readiness behavior; no duplicate setup wizard |
| About | `blank` | Minimal readable page/dialog spacing | One concise AnnoCAT paragraph, version, Apache-2.0 license, repository and safety statement |

Lower-level blocks are preferred whenever a full template would bring unrelated
structure. In particular, use `TableResizableTable`, `TableSortableTable`,
`TableSelectableTable`, `StickyColumnsHookUsage`, `ToolbarTableFilter`,
`ToolbarBulkActions`, `CheckboxListSelectAllPattern`, `TypeaheadSearchField`, and
`TokenizerOverflow` as focused references. Templates must not introduce another
router, store, request client, table engine, virtualizer, task tracker, or report
model.

Template adoption never relaxes parity. Before replacing a production surface,
compare it against the Phase 0 behavior inventory and the acceptance matrix below.
If a template cannot express an existing behavior, extend the composition using
Astryx primitives while retaining the existing behavior; do not remove or simplify
the feature merely to match the template.

### Full-application Astryx component map

Use Astryx according to the information density and interaction model of each page. Do not force every page into cards or tables merely because those components exist.

| AnnoCAT surface | Recommended Astryx composition | Important constraint |
|---|---|---|
| Global application frame | `AppShell`, collapsible `SideNav`, `TopNav`, `Layout` | The top bar carries page identity only; remove the Ready/notification control and its unused space |
| Task status in navigation | `SideNavItem` with a Lucide Tasks icon and `endContent` count `Badge`; Lucide-only status composition while collapsed | Derived from `/api/status`; clicking always opens Tasks, which is the only detailed operational surface |
| Appearance control | compact `Button`/icon action in the `SideNav` footer, `Theme` at the application root | Toggle the generated AnnoCAT theme between light and dark without duplicating the control in the top bar; persist one global preference and retain an accessible text label/tooltip when the navigation is collapsed |
| First-run choice | Standard `Dialog`, `VStack`, two clear `Button` actions, compact supporting `Text` | Keep Browse existing results available without installed sources; do not repeat the full setup wizard in the dialog |
| New Annotation | `Layout`, `FormLayout`, `Section`, a small accessible three-step indicator composed from navigation/list primitives, `SegmentedControl`, `Selector`, `CheckboxInput`, `Collapsible`, `List`, `Banner`, bottom `Toolbar` | Use one three-step flow: **Input**, **Processing**, and **Review & start**. Choose **Annotate locally** or **Open VCF for review** after input inspection, with no browser-only upload flow or duplicate importer |
| Selected VCF batch | Dense `List` with filename/path metadata, status, remove and reorder actions | Multiple files remain separate ordered runs; preserve recovery input behavior |
| Profile/source choice | profile-card `Selector`, dense selectable `List`, `CheckboxInput`, `StatusDot`, `Banner` | Minimal, Comprehensive and Custom show their compact source membership and readiness before selection. Identity comes from `/api/profiles` and `/api/sources`; changing sources changes the profile to Custom exactly once |
| Annotation review | `MetadataList`, grouped `Section`, `StatusDot`, persistent `Banner`, primary `Button` | The action is consistently **Start**. It remains disabled until the applicable backend readiness checks pass; errors stay on the Annotation page, accepted work moves to Tasks, and Tasks marks items needing attention |
| Browse Results | Dense `List` or simple `Table`, `Timestamp`, `Badge` for counts only, `EmptyState`, compact action `Toolbar` | Completed and imported reports share one library model; do not turn every report into a large dashboard card |
| Import/open result | `Button` invoking Rust native picker, `Banner` for validation errors, `Toast` for completion | Do not use a browser `FileInput` when AnnoCAT needs a real filesystem path and backend validation |
| Results workspace | `Layout`, `LayoutPanel`, `ResizeHandle`, `Table`, compact keyword `TextInput`, filter `Dialog` with `PowerSearch`, `OverflowList`, `MoreMenu`, `CollapsibleGroup`, `MetadataList` | Preserve bounded DuckDB paging, dynamic fields, complete-result selection, exports, and evidence scope; Astryx Table does not supply WGS virtualization, so AnnoCAT retains a bounded row-window adapter |
| Variant Details summary | coordinated two-column and full-width `MetadataList` groups, `StatusDot`, concise `Text`, `Tooltip`, and compact candidate/link actions | Keep sample call, population frequency, ClinVar, QUAL/FILTER and conservation immediately visible; when a phenotype ranking is active, add one neutral **Gene phenotype rank** value for the active gene and label it as gene-level rather than variant-level; `MetadataListItem` cannot span columns, so long clinical values use a separate single-column list; collapse the compact grid to one column rather than shrinking text or clipping values |
| Variant Details evidence | borderless `CollapsibleGroup`, scoped `MetadataList`, compact `List`, small `TabList`, transcript `Selector`, `OverflowList`/`MoreMenu` for links | Clinical and population evidence precede distinct gene-level **Phenotype & disease relevance** and variant-level **Tissue & regulatory context** sections, followed by transcript-dependent evidence; every value retains source, scope and release metadata without repeated prose |
| Phenotype and gene prioritization | the existing Filters `Dialog` extended with a `TabList`, local HPO `Tokenizer`, provider `Selector`, ranked `Table`, `Collapsible` explanations, `Banner`, and explicit footer actions | Use one dialog with **Rules**, **Gene list**, and **Phenotypes** tabs rather than another toolbar button, route, or nested dialog; local term lookup remains private; online ranking sends HPO identifiers only after consent and never sends a VCF, case notes, sample identifiers, candidate variants, or local paths |
| VCF-first review and FAVOR enrichment | existing Results workspace; report-action `OverflowList` with a secondary **Enrich with FAVOR** `Button`; one `Dialog purpose="form"` using `SegmentedControl`, compact `MetadataList`, `Banner`, and fixed footer actions; shared Tasks state | Convert the VCF locally and open it immediately; only a bounded selected or filtered set of normalized GRCh38 alleles may leave the computer after explicit confirmation; `ProgressBar` remains in Tasks and no report, query, detail, or task system is duplicated |
| FAVOR tissue context | one **Tissue & regulatory context** `Collapsible` inside the existing Variant Details `CollapsibleGroup`, with `MetadataList`, small `TabList`, compact `List`, `Link`, and one secondary fetch `Button` | Fetch only after an explicit per-variant or candidate action; use tabs only for related evidence views, never create a second details pane or automatically call the provider on row/transcript changes |
| Data Sources | Dense source `List` or `Table`, `StatusDot`, `Collapsible`, compact actions, install/configuration `Dialog` | Catalog and installed-source management only; active work links to Tasks and never creates download cards on this page |
| Profile installation review | form `Dialog`, dense source `List`, `StatusDot`, inline `Collapsible`, AnnoCAT `FieldSelectionGrid` composed from `TextInput` and `CheckboxInput`, `Selector`, `Banner`, fixed footer `Toolbar` | Redesign the layout while preserving profile membership, source field configuration, network/cache size, streaming mode, concurrency, locks, dependency order, and queue semantics; do not stretch Astryx `CheckboxList`, which is intended for short groups, into a searchable long-field editor |
| Tasks | `incident-console` frame as a layout reference with grouped `Table`/dense `List`, `StatusDot`, one visible `ProgressBar` per active operation, `Timestamp`, `Collapsible`, `MoreMenu`, `EmptyState` | The single owner of download/install/annotation progress and runtime controls; group by Active, Needs attention, and Completed, without copying the template's incident state model |
| Settings | Astryx settings-form layout using `Section`, `FormLayout`, `Field`, `TextInput`, `Selector`, `CheckboxInput`, `Button`, `Banner` | Paths come from `/api/paths`; Browse invokes native directory pickers; remove controls that have no backend effect |
| About | Compact `Dialog`, `MetadataList`, `Text`, outbound link actions | One concise product paragraph plus version, license, repository, local-first statement, and safety statement |
| Persistent notices | `Banner` for actionable/blocking state; deduplicated `Toast` for short success feedback | Do not use browser alerts; do not hide actionable errors only in Tasks |
| Empty/loading/error state | `EmptyState`, `Skeleton`, `Spinner`, `Banner`, retry `Button` | Keep known content visible during refresh and avoid full-page flicker |

Use the actual Astryx stable exports. In particular, use `CheckboxInput` for one
boolean field and `CheckboxList`/`CheckboxListItem` only for short grouped choices;
there is no generic `Checkbox` export. Long, searchable field catalogs use one
AnnoCAT `FieldSelectionGrid` composition shared by the Columns and retained-field
editors. Mount production dialogs and notifications under
`LayerProvider`, use `useToast` for deduplicated transient feedback, and use
`AlertDialog` for destructive confirmations. At narrow widths the Variant Details
panel may become a full-screen `Dialog`; `MobileNav` remains reserved for
application navigation.

This component map and the phased contract below supersede conflicting recommendations
in `web/astryx-prototype/ASTRYX_EVALUATION.md`. That file describes an early mockup,
not the migration authority; update or delete it when the prototype is connected to
the real application.

### Results search composition

Free-text search does **not** replace PowerSearch, and PowerSearch does not replace
free-text search. They solve different tasks:

- Keep a compact, always-visible keyword `TextInput` for fast searches across the
  displayed/searchable annotation fields. It remains server-backed, debounced and
  cancellable, and its result count is the complete-report count.
- The **Filters** button opens a form `Dialog` containing a controlled
  `PowerSearch` for typed field/operator/value clauses, gene lists, and score
  comparisons. Visible rules use the current backend's deterministic AND
  composition; comma/multi-value rules provide OR within one field. Nested Boolean
  groups are a later backend feature, not an Astryx presentation promise.
- The filter dialog edits a draft. **Apply** atomically updates the active query;
  **Cancel** leaves the current results untouched; **Reset** clears the draft.
- Saved global filter presets sit in the same dialog. Loading a preset populates
  the draft and explains unavailable fields rather than silently dropping them.
- The compact search and structured filter are composed by the existing backend
  query contract. Neither filters only the currently rendered rows.
- Use controlled sort state with Astryx `useTableSortable` so Shift-click retains
  server-backed multi-column sort priority; do not introduce a second local sort
  model.

This decision supersedes the prototype evaluation's earlier suggestion that
`PowerSearch` replace the ordinary search box. It also supersedes any mock-only
local filter implementation in the prototype.

### Variant Details information architecture

Keep the existing narrow integrated inspector and its variant-level summary. The
goal is to make more useful information immediately visible through density and
alignment, not by widening the inspector, reducing type size, or adding nested
cards.

**Identity and actions**

- Keep normalized location/allele, gene, candidate star and close action in the
  header. Long identifiers are copyable but do not dominate the heading.
- Keep resolvable primary links visible and place secondary destinations in
  `OverflowList` or `MoreMenu`. Link descriptors come from the backend allowlist.

**Compact variant-level summary**

- Use a responsive two-column `MetadataList` at ordinary desktop inspector widths.
  Each item stacks a short label over a readable value. Put long ClinVar and other
  clinical values in an adjacent full-width, single-column `MetadataList` rather
  than relying on unsupported per-item column spanning.
- Preserve the existing immediately visible fields: sample call, population
  frequency, ClinVar, QUAL/FILTER and conservation.
- Enrich sample call only when FORMAT data exists: primary sample, GT/zygosity,
  phase, DP, GQ, reference/selected-ALT reads and allele balance. Combine closely
  related values (for example `18 ref / 22 alt · 55% alt`) instead of creating a
  separate row for every number.
- Enrich population context when retained: overall AF, group-maximum AF and group,
  homozygote count and source filter status. Overall AF remains the headline value;
  ancestry counts and denominators remain in the expanded population section.
- Summarize ClinVar with classification, review stars/status and a compact conflict
  indicator. Conditions, accessions, evaluation date and citations remain in the
  expanded clinical section.
- Keep QUAL and VCF FILTER together, show one headline conservation value, and keep
  the existing compact prediction-agreement summary when its inputs are present.
  CADD and individual predictor values remain in computational evidence rather
  than crowding the top grid.
- At constrained width or high zoom, collapse the metadata grid to one column.
  Never add horizontal scrolling, truncate clinically meaningful values, or reduce
  the standard AnnoCAT text weight/size to preserve two columns.

**Evidence order and transcript scope**

1. Clinical and population evidence, both variant-level.
2. Optional **Phenotype & disease relevance**, explicitly gene-level, only when the
   report contains ranked phenotype state.
3. Optional **Tissue & regulatory context**, explicitly variant/gene-link evidence,
   only after the user has requested and saved FAVOR tissue context.
4. Transcript selector immediately before any transcript-dependent content.
5. Molecular effect, HGVS, exon/intron, biotype and transcript-quality evidence.
6. Selected-transcript predictions, labeled with the active transcript ID.
7. Stable variant-level computational evidence such as CADD, phyloP and GERP.
8. Technical details and provenance, collapsed by default.

Changing transcripts updates only transcript-scoped values. ClinVar, population
frequency, CADD, phyloP, GERP, QUAL and FILTER remain stable. A subsection with no
values shows one accurate state—source not used, no matching record, field not
retained, input field absent, not applicable or legacy state unknown—rather than a
heading, explanatory paragraph and repeated `Not reported` value.

Phenotype relevance follows the active gene rather than the genomic allele. If a
transcript change changes the active gene for an overlapping annotation, both the
compact rank and expanded phenotype section update to that gene. Never repeat a gene
rank beside every transcript of the same gene, and never imply that a high phenotype
rank makes the selected variant pathogenic.

Tissue and regulatory context is also not transcript-scoped. Changing transcripts
must not refetch or relabel QTL, chromatin, enhancer-gene or target-gene evidence as
though it were a transcript prediction. When a linked target gene differs from the
currently selected transcript gene, show both identities explicitly.

### Profile installation dialog redesign

The old modal's behavior is the parity contract, not its visual layout. Replace it
with one coherent form dialog; do not open nested field-configuration dialogs.

**Header and summary**

- Title the action clearly, for example **Install Comprehensive profile**.
- Show a compact summary of selected sources, installed coverage and total known
  network size. Mark unknown cache sizes as **measured during install**.
- State that changing the install selection affects this queue request only; it
  does not silently rewrite the named annotation profile.

**Source list**

- Present Core annotation data as one dependency group containing GRCh38 followed
  by the Ensembl transcript cache, while retaining their individual state and size.
- Use aligned dense rows for supplementary sources: name, one-line purpose,
  status, decimal network size, measured/estimated cache size and included state.
- Select all eligible profile sources by default. Installed sources remain visible
  and are skipped; active sources remain active and must not block other selected
  sources from being queued.
- Put `N retained fields · Expand to customize` in the applicable source row.
  Expansion reveals that source's field editor immediately below the row.

**Retained-field editor**

- Use the shared AnnoCAT `FieldSelectionGrid`: one search input, compact responsive
  columns of `CheckboxInput` rows, readable field names, short descriptions and the
  raw field ID as secondary text or tooltip. Astryx `CheckboxList` remains reserved
  for short groups and is not extended or forked.
- Apply the same editor to every configurable supplementary source, including
  ClinVar; do not special-case dbNSFP into a more capable dialog.
- Give each source a tri-state select-all control plus **Recommended defaults** and
  **Clear optional fields**. Do not reintroduce the removed **Required only** mode.
- Source and field choices come from the backend catalog's allowed compatibility
  contract. The server validates them again before queueing; arbitrary field names
  are never sent to fastVEP.
- Installed field sets are read-only until the user explicitly chooses to rebuild;
  active field sets are locked with a concise explanation.

**Download settings and actions**

- Keep one discoverable `Collapsible` immediately above the footer, expanded by
  default. Its label changes between **Click to collapse** and **Expand to
  customize**.
- Include the source mode selector—**Streaming** for lower peak disk use and
  **Resumable** for more temporary disk use with less lost download progress—and a
  concurrency selector from one through four.
- Concurrency limits active work, not queue length: every eligible selected source
  is queued even when the limit is lower.
- Use one concise storage sentence and the shared decimal units; do not repeat the
  explanation under every source.
- Keep a fixed footer showing selected count, total known network size and the best
  available peak/cache estimate, followed by **Cancel** and **Queue N
  installations**. An actionable `Banner` appears above the footer and focus moves
  to the first invalid or locked choice.
- Closing or cancelling discards dialog drafts and never queues, removes or rebuilds
  a source.

The dialog body owns scrolling while its header and footer remain fixed, keeping
the scrollbar inside the rounded boundary. At narrow widths or high zoom it becomes
a full-screen dialog; field choices collapse to fewer grid columns without
truncating their labels.

Astryx's `FileInput` may be used only when AnnoCAT intentionally wants browser file bytes. The desktop application currently needs native filesystem paths for VCFs, report ZIPs, output folders, resource directories, and results directories, so those controls remain buttons/fields backed by the Rust picker endpoints.

Use `Card` only for genuinely discrete summary objects, such as a small dashboard metric or first-run choice. Use `Section`, `List`, `Table`, spacing, and dividers for ordinary page structure. This avoids reproducing the current problem of multiple nested or duplicated cards.

## 5. Development and release arrangement

### During migration

- Keep one parallel React SPA isolated under `web/astryx-prototype`; do not build a
  separate prototype for each page.
- Continue using Vite on port 8799 for hot reload; Rust recompilation is not required for CSS or React changes. Proxy `/api` to the current Rust service on port 8792 so the browser sees one API contract and the same data roots.
- Do not add mock fallbacks after real API integration. If the service is unavailable, show an honest connection error rather than synthetic results.
- Keep the complete production interface available as the comparison and fallback until every baseline page passes.
- Never point the prototype at a different resources or results directory when performing parity tests.
- Do not commit `node_modules`, `.pnpm-store`, transient Vite output, or local result data.

### At release cutover

- Perform one application-wide production cutover after all baseline pages and cross-page flows pass; do not ship a half-migrated shell.
- Produce versioned static assets with a locked package manager and lockfile.
- Serve or embed the built assets from the Rust executable using the same origin as the API.
- Code-split route modules where useful and enforce a recorded compressed/uncompressed bundle budget.
- Bundle third-party license notices for Astryx, React, StyleX, Lucide and every shipped frontend dependency.
- Remove source maps and development-only routes from release packages unless intentionally retained for diagnostics.
- Confirm the release starts with no Node.js installation and with networking disabled for report viewing.

## 6. Migration sequence

### Phase 0 — Freeze and test the complete working baseline

Purpose: prevent a visual rewrite from silently losing existing behavior.

Deliverables:

- Record the behavior inventory for every production page and the cross-page flows as the parity checklist for this plan.
- Add or preserve fixtures for:
  - a 1,000+ row result;
  - a 10,000-row scale fixture;
  - a chromosome 22 result with multiple evidence sources;
  - an imported shared report;
  - unknown/dynamic evidence fields;
  - single-sample, multisample, and multiallelic VCF-derived results.
- Capture representative task snapshots for determinate download/install progress,
  indeterminate startup/index reading, retained-part replay, validation, cache build,
  annotation, pause/resume, reconnect, cancellation, failure, completion, and two to
  four simultaneous operations.
- Cover existing behavior with a small real-server browser harness:
  - continuous loading beyond the first page;
  - query reset and aborted requests;
  - numeric and text filters;
  - multi-column sorting;
  - column visibility, order, width, and persistence;
  - explicit and select-all-filtered selection;
  - candidate add/remove and Candidates view;
  - selected and filtered exports;
  - detail loading without flicker;
  - share, import, reopen, rename, and case notes.
- Record a route-by-route parity ledger containing every visible action, disabled
  state, native picker, status, error location, persistence behavior and responsive
  fallback. Every migrated page must close its ledger with no unexplained removal;
  visual rearrangement alone is not a reason to drop a feature.
- Measure current report-open, filter, sort, and page-load timings for comparison.
- Record screenshots and interaction checks at 1024×768, 1366×768,
  1920×1080, a short-height viewport, browser zoom 80/100/125/150/200%, and
  representative Windows display scaling.

Exit gate:

- The current production viewer passes the baseline suite before React is connected to real reports.

### Phase 1 — Establish the typed API and route boundary

Purpose: replace mock data without creating a parallel backend.

Deliverables:

- Define the complete route registry for New Annotation, Browse Results, an
  individual Results workspace, Data Sources, Tasks and Settings, plus About and
  First-run dialogs.
- Use the typed hash-route contract and configure the Vite `/api` proxy to the
  current service; do not add a router dependency solely for these routes.
- Give every unmigrated route an explicit legacy fallback during development;
  refresh and direct links must continue to work throughout the migration.
- Introduce one typed fetch client with:
  - same-origin base URL;
  - CSRF header handling for mutations;
  - JSON and blob responses;
  - normalized HTTP errors;
  - `AbortSignal` support;
  - no automatic retries for non-idempotent operations.
- Define TypeScript shapes from actual endpoint responses, not the mock `Variant` interface.
- Create API ownership modules for results, review, annotation, resources, tasks,
  settings and application metadata as each route needs them; do not place all
  endpoints in a Results client or create speculative wrappers in advance.
- Load `/api/runs` and open a selected real run.
- Preserve the existing production route as fallback.
- Display honest report metadata and counts; remove the synthetic 1,000-row claim.
- Add an error boundary and shared loading/error/retry presentation.
- Add the narrowly scoped in-memory session state for an annotation draft, return
  targets and Browse position; prove that it is cleared on reload and never mirrors
  server-owned records.

Exit gate:

- The prototype can open the same local and imported reports as the production viewer and never modifies report files directly.

### Phase 2 — Restore bounded WGS loading

Purpose: make the new table a real large-report viewer.

Deliverables:

- Use the existing result page endpoint, count behavior, and field catalog.
- Load bounded pages and append without duplicates.
- Add continuous loading using an observer rooted in the table scroll viewport plus a fallback check.
- Preserve loaded rows while a subsequent page loads.
- Use request generations and abort controllers to discard stale responses.
- Keep page memory bounded.
- Add an AnnoCAT-owned fixed-height row window above the bounded server pages:
  render only visible rows plus overscan, maintain top/bottom spacer heights,
  evict distant pages, preserve stable scroll position, and keep selection and
  candidate membership independent of mounted row elements. Astryx `Table` owns
  presentation but does not provide this WGS virtualization layer.
- Display loaded, matching, and total counts accurately.
- Provide loading-more, complete, error, retry, and empty states without replacing valid rows with a blank table.
- Do not render an entire WGS result in the DOM or use DOM membership as selection state.

Exit gate:

- The 1,000+, 10,000, and chromosome 22 fixtures scroll continuously with no fixed 84/100-row ceiling, duplicates, blank reset, or unbounded browser memory growth.

### Phase 3 — Restore table and column parity

Purpose: make the Astryx table usable for real heterogeneous annotations.

Deliverables:

- Generate columns from core field definitions plus the report field catalog.
- Preserve unknown future evidence fields.
- Add compact defaults for selection, candidate, chromosome, position, alleles, gene, consequence, impact, clinical evidence, frequency, and recommended scores.
- Correctly wire resizing:
  - current width state must feed the rendered column definition;
  - pointer resize updates only the affected column;
  - double-click resets to its compact default;
  - widths persist by stable column identity.
- Implement reordering:
  - drag or accessible move controls;
  - headers and cells use one canonical order;
  - order persists by stable source ID, scope, and field path;
  - missing/new fields are reconciled without discarding the rest of the layout.
- Build the Columns selector with:
  - a `Dialog` or anchored popover over the table rather than a layout-pushing panel;
  - the shared `FieldSelectionGrid` rather than `CheckboxList` or `MultiSelector`,
    because the selector needs long searchable groups, per-source tri-state
    controls, descriptions and ordered columns;
  - source-database grouping;
  - select all/none per source;
  - global search;
  - descriptions, raw keys, types, scope, and tooltips;
  - human-readable/raw-name preference;
  - recommended defaults and Restore defaults;
  - a non-blocking performance warning for unusually many enabled fields, without
    an arbitrary schema-dependent refusal.
- Restore server-side type-aware multi-sort with Astryx controlled
  `useTableSortable`, Shift-click, direction cycling, priority indicators, and
  natural-order reset. Do not use a separate local `useTableSortableState` model.

Exit gate:

- Resizing, moving, hiding, restoring, sorting, refreshing, reopening, and switching between reports all retain coherent column behavior.

### Phase 4 — Restore search, filters, and global presets

Purpose: retain fast keyword search and make PowerSearch a presentation for AnnoCAT's structured filter engine, not a replacement engine.

Deliverables:

- Preserve the compact free-text search across displayed annotation fields as a distinct, always-visible control.
- Put controlled `PowerSearch`, saved presets and typed rule editing inside the Filters dialog; do not replace either search workflow with the other.
- Expose core and dynamic evidence fields through the existing semantic field identity.
- Support current operators:
  - numeric `=`, `!=`, `>`, `>=`, `<`, and `<=`;
  - text equality, inequality, contains, and not contains;
  - comma-separated list matching for genes and other suitable text fields.
- Add `between`, `is present`, and `is missing` only when the backend compiler and tests support them.
- Treat comma-separated gene input as normalized gene symbols without requiring HPO identifiers.
- Keep draft and applied filters separate.
- Provide explicit Apply, Cancel, and Reset behavior.
- Show a bounded live match preview only if it does not create excessive queries; debounce and cancel it.
- Restore global saved-filter load, save, rename/delete, migration, and unavailable-field warnings.
- Keep prioritization presets separate from saved user filters; presets populate visible editable rules.
- Preserve missing values rather than treating them as zero.

Exit gate:

- Every existing filter works against full reports; saved filters work across compatible reports; numeric scores and gene lists return the same rows as production.

### Phase 5 — Restore selection, candidates, and exports

Purpose: preserve complete-report actions without loading all rows.

Deliverables:

- Implement the existing two selection modes:
  - explicit allele IDs;
  - every filtered result plus an exclusion set.
- Support row checkbox, header checkbox, Ctrl/Command-click, Shift-click range, and individual deselection after Select all filtered.
- Keep selection counts honest when rows are unloaded.
- Add a compact action toolbar with an overflow menu at constrained widths.
- Restore:
  - add/remove selected variants from Candidates;
  - header candidate star for the complete applicable set;
  - persistent per-row and detail candidate controls;
  - authoritative Candidates tab and count;
  - API batching, errors, disabled states, and rollback/refresh behavior.
- Restore exports:
  - selected genes;
  - all filtered genes;
  - selected rows with visible columns;
  - all filtered rows with visible columns and exclusions;
  - human-readable names and gene.iobio-compatible comma-separated symbols;
  - native Save As handoff and report-derived filenames.

Exit gate:

- Selection and candidate actions work across unloaded pages, survive reload, and export exactly the intended rows and visible columns.

### Phase 6 — Restore report actions and Variant Details

Purpose: make clinical review behavior correct before visual refinement.

Deliverables:

- Restore Back to completed runs, Rename, Case notes, Share report, and report ZIP Save As behavior.
- Fetch details through the existing bounded detail endpoint.
- Preserve the open pane and current content while another detail request loads; reject stale detail responses.
- Add candidate and close controls with stable focus behavior.
- Open rows by mouse and keyboard; visually distinguish detail focus from checkbox selection.
- Implement the Variant Details hierarchy defined above: compact variant identity,
  the responsive two-column variant-level summary, clinical/population evidence,
  transcript selection, molecular effect, scoped prediction/conservation groups,
  additional source domains, technical details and provenance.
- Preserve raw FORMAT/sample values but have Rust return bounded structured sample
  calls derived with the selected ALT index. React selects the primary sample and
  displays GT/zygosity, phase, DP, GQ, allele-specific AD and allele balance; it
  does not become a second genotype parser.
- Generate sections from real evidence and field metadata rather than a fixed score list.
- Preserve evidence scope and dbNSFP transcript alignment.
- Reset selected-transcript state when changing variants and remember it only for the correct allele.
- Ensure variant-level values remain stable and transcript-scoped values follow the selected transcript.
- Retain source, scope, field, interpretation, and release information in concise tooltips.
- Display one accurate missing state instead of repeated empty descriptions.
- Use backend-normalized, allowlisted external links when that clinical-plan API is available; until then preserve the existing validated destinations without accepting arbitrary imported URLs.

Exit gate:

- Real clinical, population, prediction, transcript, sample, and provenance evidence matches the production viewer for the golden fixtures, with no detail flicker. The summary remains readable in two columns at normal inspector widths and one column at constrained width/zoom.

### Phase 7 — Branding, generated light/dark theme, icons, responsive behavior, and accessibility

Purpose: make the replacement coherent and safe to operate at supported sizes.

Deliverables:

- Create one static AnnoCAT Astryx theme from the existing `#4057D6` brand accent
  using `defineTheme({color: {accent: '#4057D6', neutralStyle: 'cool',
  contrast: 'standard'}})`. Let Astryx derive the complete light and dark color
  scales from that single seed through its HCT color generator; do not hand-maintain
  a second palette or scatter `--color-*` overrides through components.
- Build the production theme with `astryx theme build`; runtime `defineTheme` is
  acceptable only while prototyping. Explicit semantic status/data tokens and the
  smallest necessary component overrides may supplement the generated palette, but
  they must remain centralized in the AnnoCAT theme source.
- Add one visible light/dark toggle in the `SideNav` footer using Lucide Sun/Moon
  glyphs and an accessible changing label (`Use dark mode` / `Use light mode`). It
  remains discoverable as a tooltip-labelled icon when the navigation is collapsed,
  updates the root Astryx `Theme` without reloading, and persists one global
  preference. Do not duplicate it in the top bar or Results toolbar.
- Default a new installation to light mode. Both modes must retain the same semantic
  meaning, contrast, density and hierarchy; dark mode is not a simple color inversion
  and must not introduce glowing saturated score colors or hide table boundaries.
- Use Lucide as the only glyph source:
  - pin `lucide-react` in the frontend package and lockfile;
  - render Lucide components through Astryx `Icon` or the icon props on Astryx components;
  - register Lucide replacements for every Astryx semantic icon through `registerIcons` so fallback Astryx glyphs are not mixed into migrated screens;
  - route AnnoCAT-specific icon names through one typed `AppIcon` mapping;
  - standardize ordinary icons on the 24px outline design with a 1.5 stroke width; Astryx continues to control rendered size and semantic color;
  - use Astryx selected state for navigation rather than introducing a second filled icon family;
  - selected Candidate stars may use the same Lucide `Star` glyph with `fill="currentColor"`;
  - import named icons individually; do not use Lucide's dynamic all-icon registry in the production bundle;
  - do not mix Heroicons, Astryx fallback glyphs, SVG sprites, custom path drawings, Unicode stars/arrows, emoji, or another icon library into migrated screens;
  - include the Lucide ISC and inherited Feather MIT notices in AnnoCAT's third-party notices.
- Add icons for navigation, search, filters, columns, presets, selection, candidates, export, share, notes, rename, expand/collapse, external links, statuses, warnings, errors, and empty states.
- Keep text with unfamiliar actions. Every icon-only control receives an accessible name and tooltip.
- Use overflow menus instead of wrapping action buttons over the inspector.
- Keep the table dominant when the navigation collapses.
- Preserve the inspector width when the navigation changes.
- At narrow widths, open Variant Details as an accessible drawer or stacked view; never simply hide it.
- Enforce one page scroll owner, one table scroll owner and one independently scrolling inspector; reaching the workspace must stop page scrolling and leave no blank bottom region.
- Verify table and inspector scrolling, zoom seams, long values, sticky headers, resize handles, and navigation collapse. Collapsing navigation expands the table only and does not change inspector width.
- Verify keyboard navigation, focus return, live status announcements, contrast and reduced motion in both light and dark modes at 1024×768, 1366×768, 1920×1080, a short-height viewport, zoom 80/100/125/150/200%, and representative Windows display scaling.

Exit gate:

- The Results workspace passes keyboard and screen-reader smoke tests in light and
  dark modes at representative Windows scaling, browser zoom, and narrow/desktop
  widths without clipped controls, inaccessible details or changed clinical meaning.

### Phase 8 — Results parity checkpoint

Purpose: prove the hardest page without prematurely splitting the production application.

Deliverables:

- Keep the React Results workspace in the parallel SPA after Phases 0–7 pass.
- Treat the existing Results page as the production comparison until every Phase 9 surface and cross-page flow passes.
- Identify the old Results HTML, event bindings, rendering functions and Results-only CSS for deletion at the final application cutover; do not delete the production fallback yet.
- Remove mock rows, mock transcript data, inert buttons, and prototype-only labels.
- Remove temporary adapters that duplicate final API behavior.
- Record the final JavaScript/CSS size and compare it with the prototype baseline.
- Confirm net code ownership is simpler: one Results renderer, one query path, one selection model, one candidate API, and one details implementation.

Exit gate:

- The parallel SPA opens real existing data and reports with complete Results parity; the production fallback remains intact pending Phase 9G.

### Phase 9 — Migrate remaining application surfaces incrementally

Purpose: complete the application migration without losing the non-Results workflows that make AnnoCAT usable.

The page order below minimizes risk and lets each new page reuse state already proven by the preceding page.

#### Phase 9A — Browse Results and report library

Use `table-page` as the layout reference with a dense `List` or simple `Table`;
report entries are records, not promotional cards. Copy the page rhythm and compact
toolbar only, not its sample query or table state.

Preserve full parity for:

- automatically listed completed local annotation runs;
- newest-first ordering and human-readable completion dates;
- report name, assembly, variant count, result size, local/imported identity, and completion state;
- opening and returning from a report without losing the library;
- importing an AnnoCAT report ZIP or supported existing result through the native picker;
- path containment, size/count/checksum validation and atomic publication in Rust;
- reopening imported reports after restart;
- opening the synthetic demonstration;
- empty, loading, import-in-progress, invalid-package, duplicate-import, and retry states;
- first-run entry into Browse Results without requiring databases.

Recommended composition:

- page `Layout` and `Section` for hierarchy;
- compact `Toolbar` for Import report and Open demo;
- `List`/`Table` rows with `Timestamp`, metadata, and a trailing Open action;
- `EmptyState` only when the library is genuinely empty;
- `Banner` for persistent import errors and a deduplicated `Toast` for successful import.

Acceptance gate:

- The same local and imported runs appear in both interfaces and open the same canonical report; the native picker opens once and never requires an additional browser confirmation.

#### Phase 9B — Tasks and sidebar status

Use `incident-console` as the frame-first layout reference and the Astryx progress
blocks as component references, not a Kanban board. Downloads, installations, and
annotations are stateful machine operations, not manually moved work items. Do not
copy the template's incident records, filters, grouping model, or detail types; bind
the composition directly to AnnoCAT's one authoritative task snapshot.

Preserve full parity for:

- every active download, preparation, cache build, annotation and recovery task;
- queued, running, validating, reconnecting, replaying, installing, indexing, publishing, paused, cancelling, interrupted, failed, ready and completed states;
- percentage, current chromosome/part, completed and total bytes or variants, speed, ETA and concise detail;
- Pause, Resume, Cancel/delete, retry/recover and annotation Cancel actions when supported; source Install, Update and Remove remain on Data Sources while idle;
- action-disabled states while a request is pending;
- simultaneous tasks up to configured concurrency;
- active/failed/completed summary counts;
- newest-first historical ordering;
- readable timestamps and persistent failure details;
- the Tasks sidebar indicator counting active or attention-required tasks from the same snapshot as the page;
- annotation-start errors appearing where the action occurred and updating the Tasks navigation indicator;
- polling or status refresh without duplicating cards, adding log lines, or resetting progress.
- all download and installation progress cards formerly shown on Data Sources;
- the only Pause, Resume, Cancel and delete, and other runtime task controls in the page UI.

Recommended composition:

- `TabList` or collapsible grouped sections for Active, Needs attention and Completed;
- dense `Table`/`List` rows;
- `StatusDot` for state and one visible, accessible `ProgressBar` for every active
  operation that lasts long enough to track;
- a determinate `ProgressBar` with its percentage label when the server supplies a
  trustworthy completed/total ratio, and an indeterminate `ProgressBar` while the
  total is genuinely unknown; do not replace an unknown long-running operation with
  a tiny spinner or stack multiple bars for the same task;
- a stable progress line immediately adjacent to the bar containing the current
  phase and, when applicable, chromosome/part, completed and total decimal bytes or
  variants, current rate and ETA. Omit unavailable measurements instead of showing
  invented zeroes;
- progress color communicates task state consistently: accent while active, warning
  while paused/reconnecting or otherwise waiting for action, error when failed, and
  success only when complete. Text and `StatusDot` remain present so color is never
  the only signal;
- `Timestamp` for updated/completed time;
- visible primary action plus `MoreMenu` for secondary/destructive actions;
- `Collapsible` detail for long technical messages;
- `EmptyState` for no tasks.

The task row has one stable height and identity while it updates. Polling changes
the existing row's state, label, measurements and bar value in place; it must not
remount the row, reset the bar, reorder an active operation on every tick, duplicate
the task, or append a new visible log row. Completed/failed transitions may move the
row between groups once. Pausing freezes the last valid determinate value; resuming
continues the same task identity. A reconnect, retained-part replay, validation or
cache-build phase updates the phase text without pretending that already completed
overall work returned to zero.

Use one task view model produced from `/api/status` or `/api/tasks`. The Tasks page and Tasks navigation indicator consume the same summary; neither keeps a parallel status tracker.

Remove the Ready/notification control from the top bar. Use Astryx `SideNavItem.endContent` on the Tasks item:

- no badge when there are no active or attention-required tasks;
- a compact neutral count badge when tasks are active;
- an error count badge when one or more tasks need attention, taking visual precedence over the active count;
- the full breakdown remains on the Tasks page rather than in a sidebar popover.

Astryx hides `endContent` when the SideNav is collapsed. Preserve status discoverability in collapsed mode through `AppIcon`, composing Lucide's Tasks glyph with a small Lucide status glyph inside the same SVG boundary. Its tooltip/accessibility label must announce, for example, `Tasks, 2 active` or `Tasks, 1 needs attention`. Do not draw custom SVG paths or add a separately positioned DOM badge over the navigation rail.

Tasks is the single detailed operational surface. Data Sources and New Annotation may show a compact state such as Queued, Downloading, Installing, Paused or Needs attention, but their action is **View task**. They do not render another progress card or a second set of Pause/Resume/Cancel controls.

Acceptance gate:

- Task identity and state agree in Tasks and the relevant Data Source or Annotation context; Data Sources and Annotation provide **View task**, while Tasks alone provides Pause, Resume, Cancel/delete and other runtime controls. The sidebar count matches the same task summary.
- Every long-running active task has one continuously updating bar. Determinate bars
  agree with authoritative server completed/total values, indeterminate bars are
  used only when no reliable total exists, and pause/resume/reconnect/replay/build
  transitions preserve task identity and truthful progress without flicker or reset.

#### Phase 9C — Data Sources and installation workflow

Use the category/profile hierarchy from `library` and the compact configurable-row
hierarchy from `settings-sidebar`, without importing either template's local state.
Use one source row per catalog entry. Data Sources owns catalog, installation intent,
configuration, versions, installed size and removal. Tasks owns execution progress
and runtime control. An active source row shows only a compact state and **View
task** link; it must not render a progress card.

Preserve full parity for:

- Core annotation data as one coherent dependency with GRCh38 followed automatically by the Ensembl transcript cache;
- Comprehensive and Minimal profiles, profile contents, installed coverage, and install-all actions;
- installable sources sorted before expandable pending/catalog-only sources;
- installed, not installed, incomplete, update available, queued, downloading, paused, preparing, ready, interrupted, failed and removing states;
- source download/network size and measured cache-on-disk size using consistent decimal units;
- Install, Configure fields, Update/check for update and Remove when the source is idle;
- a View task action whenever a download, preparation, removal or recovery task exists;
- no duplicate current bytes, speed, percentage, phase, chromosome/part or runtime controls; those appear in Tasks;
- resumable and pure-streaming source modes;
- configurable concurrency from one through four, with every requested source queued even when concurrency is lower;
- dbNSFP and supplementary retained-field configuration, group select-all, descriptions, defaults, installed-field locking and rebuild requirements;
- the redesigned profile installation dialog defined above, including core and supplementary sources, network/cache information, inline retained fields, download safety and concurrency;
- expandable pending sources;
- rolling versus pinned version/update behavior;
- source-specific failure details and recovery actions;
- resource directory display and navigation to Settings.

Recommended composition:

- compact profile `Section` above the catalog, with profile name, contents and one install/review action;
- dense source `List` or `Table`, not independent dashboard cards;
- `StatusDot`, compact installed/version/size metadata, and one context-appropriate action area in each source row;
- `Collapsible` row details for descriptions, versions, cache identity and failures;
- one form `Dialog` for profile review with inline retained-field configuration; do not open a nested source-field dialog;
- dense source rows, responsive shared `FieldSelectionGrid` groups, `Selector`, `Collapsible`, `Banner`, and fixed dialog footer;
- `Banner` for an actionable page-level failure; deduplicated `Toast` for successfully queued actions.

The source row and Tasks entry use the same server snapshot, but have different responsibilities. The source row summarizes availability and installation state; the Tasks entry presents live progress, detail and runtime controls. Source configuration is saved before queueing; an already active source is skipped or left active rather than causing the rest of the profile request to fail.

Acceptance gate:

- Every source can be queued, configured, updated and removed from Data Sources; View task reaches the corresponding operational entry; Pause/Resume/Cancel in Tasks updates the source row immediately; profile installs queue all eligible sources and honor concurrency.

#### Phase 9D — Settings and native directory pickers

Use `settings-sidebar` as the layout reference, adapted to a local desktop
application rather than an account-management page. Retain only its section,
editable-row and responsive navigation composition; AnnoCAT's settings API remains
the sole state owner.

Preserve full parity for:

- resource directory;
- downloads directory;
- results directory;
- Browse controls for mutable directories;
- native picker behavior, cancellation and error reporting;
- paths with spaces and non-ASCII characters;
- installed resources and existing results remaining visible after an allowed directory change;
- startup setup-prompt preference;
- default annotation profile;
- source input mode: resumable or pure streaming;
- concurrent installations from one through four;
- one persisted application appearance mode, controlled by the single light/dark
  button in the SideNav footer;
- online phenotype services and FAVOR enrichment disabled until the user explicitly
  enables each service and accepts its concise provider-specific privacy disclosure;
- Reset preferences with clear confirmation and deterministic defaults;
- preferences surviving restart;
- unavailable/nonfunctional settings being omitted rather than displayed.

Recommended composition:

- one scrolling page of `Section` groups rather than separate cards for every setting;
- `FormLayout` with `Field`, read-only `TextInput`/path display, and trailing Browse `Button`;
- `Selector` for profile, source mode and concurrency;
- `CheckboxInput` for startup behavior and individually enabled online services;
- `Banner` for errors that require action and `Toast` for saved preferences.

Settings owns online-service availability, disclosure acknowledgement, and provider
health—not a second phenotype or FAVOR workflow. Phenotype terms and ranking are
entered and run only from the Results filter/prioritization dialog; FAVOR requests
are configured and started only from the relevant Results actions. FAVOR needs no
account, API-key, provider selector, or arbitrary URL setting in the first release.
Use reviewed HTTPS endpoints with bounded response contracts so a malformed or
incompatible service cannot enter report state.

Do not use `FileInput` or a browser directory attribute for these controls. Invoke the existing Rust picker APIs so the release receives usable local paths.

Acceptance gate:

- Every displayed setting changes real backend/application behavior, persists, and never hides installed resources or runs because the UI accidentally started with a different home directory.

#### Phase 9E — New Annotation wizard and recovery

Use one short three-step mental model: **Input**, **Processing**, and **Review &
start**. Astryx does not currently provide a purpose-built stepper that should
dictate the workflow, so compose a small accessible progress indicator from
navigation/list primitives and shared tokens; do not add a general wizard framework.
Output remains part of the final review instead of consuming a separate step.

Preserve full parity for:

- choosing one or multiple VCF, VCF.GZ or BGZ files through the Rust native picker;
- choosing **Annotate locally** or **Open VCF for review** with one two-item
  `SegmentedControl` after input selection; this is a run mode, not a profile or data
  source, and the VCF is not picked a second time from Browse Results;
- inspecting each selected input immediately and showing filename, size, detected
  assembly, sample count/names and available variant-count metadata before the user
  selects a processing mode;
- selected file list, order, removal and one-run-per-file behavior;
- interrupted annotation recovery and recovery-input selection;
- profile selection and available source selection;
- profile cards for **Minimal**, **Comprehensive**, and **Custom** that show compact
  source membership and readiness before selection rather than hiding membership in
  a later dialog;
- automatically selecting all ready sources belonging to a profile;
- changing to Custom when the user changes profile contents;
- Core annotation readiness and source-specific readiness;
- direct navigation to Data Sources when something is missing;
- using the configured Results directory by default and showing it in Review;
- an output-directory override through the native picker under Advanced rather than
  duplicating the main Settings control;
- optional annotated VCF, off by default;
- complete review of files, output, profile, sources and readiness;
- backend validation before start;
- actionable start failures remaining on the Annotation page and updating the Tasks attention indicator;
- batch queue behavior and annotation cancellation;
- accepted work appearing in the existing Tasks owner, with the shared navigation
  count/status and completion notification, instead of keeping a second progress
  display in the wizard;
- no uploads or unintended downloads before review;
- step navigation preserving entered state.

For **Open VCF for review**, reuse the same Input and Review state. Hide the
profile/source controls because they are not applicable rather than preserving a
second inactive source selection. Offer one subordinate `CheckboxInput`, **Add local
gene and consequence annotations**, when Core annotation data is ready. It is on by
default when available and runs the existing fastVEP path with GRCh38 and the Ensembl
transcript cache but with an empty supplemental-provider set. It must not select,
install or contact ClinVar, dbNSFP, dbSNP, gnomAD, CADD, PhyloP, SpliceAI or any other
supplementary source.

When Core is unavailable, disable that checkbox with a concise **Set up core data**
link while leaving unannotated VCF review available. With the checkbox off, Review
states that the file will be converted locally to a `vcf-only` report. With it on,
Review states that AnnoCAT will first add local gene, transcript, consequence and HGVS
annotations and create a `core-consequences` report. Both paths use the same final
**Start** action and the same Tasks lifecycle. Do not add an in-place post-open
fastVEP upgrade in the first release.

FAVOR is not offered in this wizard and no network request occurs before the Results
workspace is open. **Annotate locally** remains the default whenever local annotation
is ready.

AnnoCAT supports GRCh38 only. Read reference/contig metadata from the VCF when it is
available. Reject a VCF that explicitly declares GRCh37 or incompatible reference
contigs with an actionable GRCh38-only error; do not offer liftover or an assembly
`Selector`. If the header cannot establish the build, show one warning `Banner` and
require a single `CheckboxInput` confirmation that the file uses GRCh38 before
continuing. A missing header is never treated as implicit GRCh38.

Recommended composition:

- `Layout` with a compact accessible step list and `FormLayout` content;
- `Section` per step rather than nested cards;
- native-picker `Button` plus selected-file `List`;
- two-item `SegmentedControl` for local annotation versus local VCF review;
- profile-card `Selector` for Minimal, Comprehensive and Custom, with compact source
  membership and readiness visible on each item;
- dense `CheckboxInput` source rows with `StatusDot`, shown only for local annotation;
- one subordinate `CheckboxInput` for optional core-only fastVEP consequences in VCF
  review mode;
- `Collapsible` advanced output options;
- `Banner` for readiness and start errors;
- `MetadataList` for Review;
- persistent bottom `Toolbar` with Back and Continue/Start.

Acceptance gate:

- A real single-file, multi-file and recovered annotation starts with the same validated inputs and outputs as production, and an unavailable source produces a useful error without navigating the user away unexpectedly.
- A GRCh38 VCF can instead open as a local report through this same wizard without
  fastVEP or installed data sources; an explicitly non-GRCh38 VCF is rejected, and a
  VCF with unknown assembly requires confirmation without presenting unsupported
  build choices.
- When Core is ready, the same VCF-review path can optionally run provider-free
  fastVEP and expose gene, transcript, consequence, HGVS and protein-effect fields;
  when Core is unavailable, the disabled option explains how to install it without
  blocking ordinary VCF review.
- Starting any path creates one existing Tasks entry, resets the accepted wizard
  state, and adds the completed report to Browse Results without unexpectedly
  navigating the user during a long run.

#### Phase 9F — First-run and About

First-run remains a small decision dialog, not a second source-management interface.

Preserve full parity for:

- showing setup when core annotation data is unavailable and the preference permits it;
- Browse existing results without installing sources;
- Set up local annotation opening the profile installation flow;
- Not now dismissing without corrupting readiness state;
- the dialog returning when required data is later removed;
- AnnoCAT branding consistent with the rest of the application.

About remains deliberately concise:

- what AnnoCAT does;
- portable and local-first explanation;
- existing report ZIPs can be viewed without sources;
- version;
- Apache-2.0 license;
- repository link;
- research/professional-review safety statement.

Recommended composition:

- standard `Dialog`, `Heading`, `Text`, `MetadataList`, link `Button`, and close icon;
- no duplicated source list, profile configuration or long documentation inside either dialog.

Acceptance gate:

- First-run follows real readiness and preference state; About is keyboard accessible, concise, and has valid links and version metadata.

#### Phase 9G — Final application cutover

Cut over the complete React application—including `AppShell`, navigation and the Tasks status indicator—once every destination and cross-page flow is React-backed and accepted. A deliberately bridged old screen is acceptable during development, not at this gate.

Preserve full parity for:

- New annotation, Browse results, Data sources, Tasks, Settings and About navigation;
- selected-page state and page title/breadcrumb;
- collapsible navigation with persisted preference;
- content using all freed width when navigation is collapsed;
- no overlap between navigation toggle, toolbars and detail drawers;
- no Ready/notification pill or task popover in the top bar;
- active or attention-required task count beside Tasks in the expanded sidebar and a status-marked Tasks icon when collapsed;
- the persisted light/dark button in the SideNav footer, with the same control
  available as a labelled icon when the navigation is collapsed;
- direct entry into imported reports and first-run actions;
- browser refresh restoring a valid route instead of a blank prototype;
- keyboard and narrow-width navigation.

Recommended composition:

- one `AppShell` and one `SideNav` for the entire application;
- a route-aware page outlet;
- `TopNav` with page identity only;
- `SideNavItem` for Tasks with supported `endContent` and icon props rather than custom absolute positioning;
- a responsive drawer/menu supplied by the same navigation state;
- Lucide glyphs rendered through Astryx `Icon` and the shared `AppIcon` mapping for every navigation item.

Exit gate:

- Every navigation item reaches a working page, refresh works on every supported hash route, the ordinary launcher serves the complete React build, and the legacy application shell, page renderers, event bindings and obsolete CSS are removed together.

#### Cross-page flow contract

Test these as complete user journeys rather than isolated pages:

- First-run → Set up local annotation → profile install → Tasks → return to the intended setup/annotation destination.
- First-run → Browse existing results → import or open a report → Results.
- Annotation with missing sources → Data Sources → queue install → Tasks → return to the unchanged in-memory annotation draft.
- Browse → Results → Back restores the library state and scroll position.
- Native import completes by opening the imported run directly; it does not require finding it again in Browse.
- Completed annotation opens from either Tasks or Browse and resolves to the same Results route.
- Data Source → View task → Back returns to the same source row.
- Changing an allowed resource/results directory refreshes source/run discovery without starting a second application home or losing visibility of existing data.

Return targets are navigation hints only. The destination reloads authoritative
server state; it does not receive a copied source, task or report model.

For every Phase 9 surface:

- add parity tests before changing the default route;
- reuse current Rust endpoints and canonical state;
- migrate one surface at a time;
- record its old DOM renderer, event bindings and obsolete CSS for deletion at the final Phase 9G cutover;
- keep installed resources, active downloads, current settings and existing results untouched;
- reject any new client-only state tracker that disagrees with the server snapshot.

### Phase 10 — Staged clinical-review improvements

Do not implement the v3 clinical plan wholesale. Its correctness work becomes the
first clinical-review gate; new data products and analytical workflows remain
separate later gates. The old plan's `web/src/app.js`/`style.css` file assignments
are historical and do not override the React ownership in this plan.

#### Phase 10A — Core clinical correctness

Implement for the first clinical-review release:

- Treat clinical-plan WP0 as already owned by Phase 0 of this migration.
- Add one backend source-completeness contract with the machine-readable states
  `source-not-used`, `no-match`, `field-not-retained`, `input-field-absent`,
  `not-applicable`, `present`, and `legacy-unknown`. React maps those states to one
  shared presentation vocabulary and never guesses from empty values.
- Return selected source IDs/releases, retained-field contract identities,
  transcript/cache/fastVEP identity and report creation metadata in bounded detail
  provenance.
- Return structured sample calls from Rust using FORMAT plus the row's selected ALT
  index. Add a Primary sample `Selector` only when multiple samples exist; do not
  introduce pedigree or case setup.
- Complete the Variant Details structure from Phase 6 and backend-normalized,
  allowlisted external links.
- Complete the Filters `Dialog` with controlled `PowerSearch`, typed operators,
  explicit Apply/Cancel/Reset and global saved filters while retaining the separate
  compact keyword search.
- Initially ship only two transparent prioritization presets: **Rare potentially
  functional** and **ClinVar reported**. A preset expands into visible editable
  rules and never classifies or adds candidates.
- Apply clinical-plan WP11 compatibility, security, cancellation, accessibility,
  recovery and performance tests continuously; there is no later hardening pass
  that excuses an unsafe earlier milestone.

Gate 10A passes only when single-sample, multisample and multiallelic fixtures prove
allele-specific calls; transcript fixtures prove evidence scope; missing values are
not converted to zero; filters/presets return the expected complete-report rows;
and current compatible reports remain readable.

#### Phase 10B — Enhanced source contracts, independently gated

Clinical-plan WP3 does not block Phase 10A. The existing gnomAD and ClinVar
contract-v1 caches remain usable and are described honestly as incomplete when a
new optional field is unavailable.

- Promote gnomAD FILTER and group-maximum AF/population only after exact exome and
  genome headers are verified against the selected release.
- Add useful group count/denominator/homozygote fields only after measuring cache
  size and viewer value. Never synthesize AF zero from missing data or AN zero.
- Promote ClinVar conflict state and add stable accessions, evaluation date,
  review-star inputs and bounded citations only when present in the selected source
  representation.
- Version every retained-field contract and include it in cache identity. Changing
  the field set is an explicit rebuild; an AnnoCAT update never silently relabels an
  old cache as complete.
- Do not begin this gate until cache delivery, resume, compatibility, update and
  storage behavior are reliable enough that additional fields do not make setup
  regressions harder to diagnose.

#### Phase 10C — Portable review state

Add one bounded, checksummed review-state contract only after candidates, filters
and the Results workspace are stable.

- Store report candidates, applied report-filter snapshot, column layout, primary
  sample and later phenotype selections.
- Keep the global saved-filter library application-global; do not copy it wholesale
  into each report.
- Exclude case notes from sharing by default. Include them only through a separate
  explicit privacy choice recorded in the package manifest.
- Import legacy candidates and notes without writing two authoritative candidate
  stores indefinitely. Migration is atomic and canonical result files remain
  immutable/readable if it fails.

Do not introduce SQLite solely for this bounded overlay. Reconsider a database only
if later per-variant notes, collaboration or audit history demonstrate that the
atomic JSON model is inadequate.

#### Phase 10D — Private phenotype selection with optional online ranking

Extend the existing Filters form `Dialog` with a `TabList` containing **Rules**,
**Gene list**, and **Phenotypes**. Do not add another Results-toolbar button, page, or
nested dialog. The Phenotypes tab uses Astryx `Tokenizer`, not `Typeahead`, because
users select multiple phenotype terms. Accept ordinary phenotype phrases and comma-,
semicolon-, or newline-separated lists. Search the local index by preferred label,
synonym and common wording; suggestions show the human-readable name first, why it
matched (preferred name or synonym) second, and the HPO identifier only as secondary
provenance. Selected tokens display names, not codes, so users never need to type or
remember an HPO identifier.

- Ship a versioned local HPO term index for private autocomplete, with required
  attribution and source/version metadata.
- Keep suggestions deterministic and reviewable: rank exact labels first, then
  synonyms and fuzzy matches; optionally expose broader/narrower HPO terms as
  clearly labelled refinements. Never silently select a term, infer a symptom from a
  report, or treat a suggested phenotype as observed in the patient.
- Treat gene ranking as a separate provider contract. Start with a transparent,
  versioned local direct-association baseline whose distribution terms have been
  reviewed.
- Use Monarch as the preferred optional online **phenotype similarity and evidence**
  provider. Rank the selected phenotype profile against human genes and diseases,
  then retain matched terms, association scope, source links, and release provenance.
- Offer Phen2Gene only as an optional comparison ranking. Its documented REST API
  accepts HPO identifiers and returns ranked genes, but its provider-native score is
  displayed separately from Monarch and is not treated as a newer evidence release
  or pathogenicity probability. Never submit clinical notes through Doc2HPO.
- Keep Phenolyzer out of the default provider list. It may be considered later only
  as an explicitly experimental compatibility provider after its API stability,
  privacy behavior, and commercial-use terms are resolved.
- Treat the gene.iobio/Phenolyzer interaction as UX reference only: readable
  autocomplete, removable selected terms, a ranked gene table, explicit selection,
  and gene-list export. Do not call the iobio service, scrape its UI, reuse its cached
  rankings, or inherit its academic/research-only service restrictions.
- Before enabling any hosted provider in a release, record its service terms,
  permitted use, privacy/contact policy, response contract and availability test.
  An open-source client or algorithm license does not by itself grant unrestricted
  use of somebody else's hosted endpoint; Monarch evidence also retains the license
  and attribution of its underlying source where supplied.
- Run ranking only after **Prioritize genes**. It creates a removable visible
  ranked-gene constraint in the existing query engine and never replaces **All
  variants**, hides the unfiltered result, or creates candidates.
- Present the result as **Suggested genes**, with readable gene symbol/name, native
  provider rank, selected phenotype matches available from the local HPO association
  data, linked disease evidence available from Monarch, and an expandable **Why this
  gene?** explanation. Applying selected suggested genes is a separate explicit
  action; suggestions do not automatically filter variants or create candidates.
- In the Phenotypes tab, compose the workflow from:
  - a `Tokenizer` for human-readable term entry and removable selected-term tokens;
  - an ordinary suggestion `List` showing preferred label, synonym match reason, and
    secondary HPO ID;
  - a compact `Collapsible` **Ranking method** area with a `Selector` for Local,
    Monarch, and—when enabled—Phen2Gene;
  - a concise `Banner` immediately above the action only when the selected provider
    is online;
  - a primary **Prioritize genes** `Button`, progress/cancel state, and a ranked
    `Table` containing selection, rank, gene, query-relative match, direct/inferred
    scope, matched phenotypes and related conditions;
  - a row `Collapsible` labelled **Why this gene?** for matched and unmatched selected
    terms, condition links, evidence sources and provider release;
  - one fixed dialog footer with **Cancel**, **Clear phenotype ranking**, and **Apply
    selected genes**. Applying creates the ordinary removable gene constraint used by
    the canonical query engine.
- Keep the provider-native rank visible as `#n of total` and, when useful, a
  query-relative percentile. Put the raw algorithm score in the explanation rather
  than presenting it as confidence. Do not color phenotype rank red/amber/green and
  do not average, normalize together, or choose a winner across providers.

**PowerSearch integration**

- Register phenotype ranking as typed fields in the same semantic field registry and
  backend filter compiler used by ordinary report fields; do not build a second
  phenotype-only query engine or expand thousands of genes into a browser-side `IN`
  list.
- Add a **Phenotype profile…** suggestion/action to PowerSearch. Choosing it focuses
  the **Phenotypes** tab and local `Tokenizer`; it does not send a request or apply a
  filter until terms are confirmed, ranking finishes, and the user clicks **Apply
  selected genes**.
- After a profile is applied, represent it in PowerSearch as one editable compound
  rule such as **Phenotype-ranked genes · Monarch · top 100**. Removing the rule
  restores the prior result set without deleting the stored ranking; **Edit** returns
  to the Phenotypes tab.
- Expose typed PowerSearch fields and operators:
  - **Gene phenotype rank**: `is ranked`, `is not ranked`, `=`, `!=`, `<`, `<=`, `>`,
    `>=`, and `between`, with helper text that rank 1 is highest;
  - **Matched phenotype**: `contains any`, `contains all`, `does not contain`, using
    the same human-readable HPO suggestions rather than requiring codes;
  - **Related condition**: `contains` and `does not contain` using readable disease
    labels and stable identifiers in secondary metadata;
  - **Phenotype association scope**: `is direct` or `is inferred`;
  - **Phenotype ranking provider**: `is` or `is not`, limited to providers actually
    stored for the report.
- Never expose a generic cross-provider raw-score comparison. A provider-native score
  may be displayed in **Why this gene?**, but PowerSearch uses rank, match identity,
  scope and provider so its meaning stays deterministic.
- Compile these rules against a bounded per-report gene-rank relation owned by Rust
  and joined by stable HGNC identity in DuckDB. The immutable report Parquet remains
  unchanged, full-report paging/selection continues to work, and imported rankings
  can be queried without repeating an online request.
- Global saved filters may store generic rules such as **Gene phenotype rank <= 100**,
  but case-specific HPO terms and provider results remain in report review state and
  are excluded from a global preset unless the user explicitly chooses to include a
  named reusable phenotype profile. Loading a rank-dependent preset without an
  active profile produces one actionable unavailable-field message, not zero silent
  matches.
- Keep comma-separated gene-list import/export as a provider-independent path.
- Online calls are explicit opt-in and independently cancellable. The confirmation
  lists the human-readable terms and HPO IDs that will leave the computer and states
  that the provider can observe the request and network metadata. Opening the dialog,
  searching local HPO terms, changing providers, or reopening a report never sends a
  request.
- Route provider calls through one bounded Rust client rather than directly from the
  browser. Require an allowlisted HTTPS host, timeouts, cancellation, response-size
  and result-count limits, schema validation, and a provider health/error state.
- Normalize every result into one provider-independent contract containing provider,
  provider/release version when supplied, query HPO IDs, timestamp, ranked HGNC gene
  identity, provider-native rank/score, association/evidence links, warnings, and
  whether a gene was directly associated or inferred. Preserve native scores as
  provider-specific values; never compare them across providers.
- Cache the normalized response locally with its provenance. Shared/imported reports
  never silently repeat an online request; included phenotype state and online
  provenance follow the explicit review-state sharing choice from Phase 10C.
- If Phen2Gene or Monarch is unavailable, local autocomplete, local direct-association
  ranking, imported gene lists, and the unfiltered **All variants** view continue to
  work normally.

**Variant Details integration**

- Do not add a phenotype section when the report has no active or stored phenotype
  ranking; the Filters dialog remains the single place to create or change one.
- When ranking exists for the active gene, add one compact neutral summary item:
  **Gene phenotype rank** with `#n of total`, provider, and a tooltip stating that it
  is gene-level, query-specific, and not variant pathogenicity.
- Insert one **Phenotype & disease relevance** `Collapsible` after variant-level
  clinical/population evidence and before **Transcript & molecular effect**. Use a
  compact `MetadataList` for active gene, provider, query timestamp/release and rank;
  dense `List` groups for matched selected phenotypes and related conditions; and a
  **View all suggested genes** link that returns to the Phenotypes tab of the same
  Filters dialog.
- Label exact, ancestor/descendant, direct and inferred relationships with neutral
  text, not clinical severity colors. Show selected phenotypes not represented in the
  provider evidence once, under **Not matched**, without treating absence of an
  annotation as evidence that the patient lacks the feature.
- Scope the section to the active transcript's gene. If another transcript belongs to
  another overlapping gene, update the rank and evidence together; variant-level
  ClinVar, population frequency, CADD, conservation, QUAL and FILTER remain stable.
- If both Monarch and Phen2Gene results were saved, show Monarch as the primary
  evidence view and a compact **Comparison ranking** row for Phen2Gene. Never combine
  the two numbers.
- Expose optional `phenotype_gene_rank`, `phenotype_provider`, and
  `phenotype_matched_terms` columns through the ordinary Columns selector. They are
  disabled by default except `phenotype_gene_rank` when a phenotype constraint is
  active. Variants in the same gene share the gene rank; secondary table sorting
  continues to use explicit variant evidence selected by the user.

#### Phase 10E — VCF-first reports, bounded FAVOR enrichment and tissue context

Add a lightweight path for users who have a VCF but do not have local annotation
sources installed. It is the **Open VCF for review** mode already defined in Phase
9E, not a second importer or annotation page. Rust converts the VCF into the same
canonical, DuckDB-queryable Parquet report family. With **Add local gene and
consequence annotations** off it produces a `vcf-only` report without fastVEP or
installed data sources. With the option on it reuses the provider-free core fastVEP
path and produces a `core-consequences` report. Both open in the existing Results
workspace and use the same report/query/task contracts.

This phase extends existing owners rather than introducing another feature stack:

- Phase 9E owns input mode, native file picking, GRCh38 confirmation and report
  creation;
- the existing Results query/selection/filter state owns enrichment scope;
- the Rust service owns provider calls, validation, bounded responses and atomic
  sidecar publication;
- Tasks owns queued/running/paused/failed/completed state and all progress bars;
- the semantic field registry and DuckDB path own table fields, filtering, sorting,
  paging and exports;
- the existing Variant Details hierarchy owns per-variant tissue display; and
- the existing report package owns optional sharing/import of complete sidecars and
  provenance.

Do not add a FAVOR route, page-level store, query engine, task tracker, report type,
or Data Sources installer. Implement the provider client only when this phase begins,
after the reused Results, Tasks, New Annotation and report-sidecar contracts pass
their earlier gates.

**Local import contract**

- Keep VCF conversion and optional core annotation as two modes of one backend run
  contract. Core mode may require only the verified GRCh38 reference and Ensembl
  transcript cache; an empty supplemental-source list must remain valid and must not
  synthesize provider readiness requirements.
- Enforce the Phase 9E GRCh38-only contract. Persist whether GRCh38 was detected from
  the header or explicitly confirmed by the user. Reject declared GRCh37 and other
  incompatible builds; do not add build selection, liftover, or a dormant GRCh37
  code path. FAVOR remains disabled unless the report has confirmed GRCh38 identity.
- Split multiallelic records into stable allele rows while retaining the source-row
  identity and ALT index needed to reconstruct provenance. Preserve normalized
  chromosome, position, REF and ALT, the original ID, QUAL, FILTER, safe INFO fields,
  and sample/FORMAT values required for genotype, zygosity, depth, allele balance,
  phase and quality. Preserve these locally even though none is sent to FAVOR.
- Build field metadata from the actual VCF header and report schema. The table,
  Columns selector, exports and PowerSearch show only fields that exist; unknown INFO
  fields remain dynamic rather than being discarded or forced into a fixed schema.
- State the limitation plainly: before enrichment, filtering can use VCF fields and
  annotations already present in the input. Gene, transcript, consequence and
  external evidence filters are unavailable unless those values were already in the
  VCF. Do not show empty annotation columns or imply that VCF conversion performed
  biological annotation.
- A `core-consequences` report additionally exposes the gene, transcript,
  consequence, HGVS and protein-effect fields actually returned by fastVEP. It does
  not imply that supplemental clinical, population, prediction or conservation
  evidence was installed or queried.

**Results-page flow**

- Identify a `vcf-only` report with ordinary report metadata such as **VCF fields
  only**. Do not add a decorative status banner, a new route, or a FAVOR tab.
- Put one secondary **Enrich with FAVOR** `Button` in the existing report-action
  `OverflowList`. At constrained widths the existing overflow behavior moves it into
  `MoreMenu`; it must not compete with the row-selection toolbar or recreate its
  overflow problems. `MoreMenu` is the responsive fallback, not the only place the
  primary enrichment entry point is exposed.
- Keep the action discoverable on a `vcf-only` report even when the current result is
  too large. Open one `Dialog purpose="form"`; do not navigate away or nest another
  dialog. A two-item `SegmentedControl` offers **Selected variants** and **Filtered
  variants**. Disable only an ineligible item and explain why beside the control, so
  selected variants remain usable when the filtered result is too large.
- The same action may appear on an ordinary locally annotated GRCh38 report when it
  lacks FAVOR fields. It requests only missing alleles/fields and keeps similarly
  named local evidence separate. It is not presented as a replacement for fastVEP or
  as proof that the local profile is incomplete.
- Start with an AnnoCAT safety cap of 1,000 eligible alleles per online job. The
  effective limit is the lower of that cap and the active provider contract; do not
  infer eligibility from loaded DOM rows or the FAVOR web uploader's file-size limit.
  Keep the cap in the provider capability record so it can be changed after measured
  API testing without changing UI logic. FAVOR's current public batch contract allows
  up to 10,000 references, but that provider maximum does not justify making 10,000
  the initial AnnoCAT default.
- Use a compact multi-column `MetadataList` in the dialog for provider, confirmed
  **GRCh38**, authoritative scope count, eligible, unsupported/skipped and
  already-enriched counts, and expected request batches. Do not add a provider
  `Selector`, assembly `Selector`, or field picker: the first contract is FAVOR,
  GRCh38 and `depth=standard`.
- Use one persistent `Banner status="info"` for the concise privacy disclosure. It
  names the exact coordinate/allele fields sent and states that FAVOR can observe
  submitted alleles, IP address and request timing. Do not require a separate
  acknowledgement checkbox; pressing the labelled primary action is the explicit
  consent event.
- On first use, that same action records the provider disclosure acknowledgement,
  enables FAVOR, and starts the requested task atomically; do not make the user visit
  Settings first. Settings can subsequently disable FAVOR. If provider health or the
  anonymous contract is unavailable, the same dialog uses one actionable `Banner`
  and does not attempt or offer authentication.
- Use a fixed dialog footer with secondary **Cancel** and one primary action labelled
  with the operation and count, for example **Enrich 214 variants**. `isLoading`
  lasts only until the existing Tasks owner accepts the job. Results then shows a
  compact **View task** `Link`; `ProgressBar` remains exclusively on Tasks.
- Completion refreshes the existing report schema and query once and uses the shared
  deduplicated toast. Keep current rows visible during that refresh so adding FAVOR
  fields cannot reproduce the empty-report flicker.

**FAVOR provider boundary**

- Use FAVOR as the first broad online provider. Its GRCh38 catalog supplies variant
  category, allele-frequency, ClinVar, integrative/protein scores, conservation and
  regulatory annotations; it complements the locally retained call/genotype data.
  Treat FAVOR's all-SNV and observed-indel coverage as an eligibility rule, not as a
  promise that every VCF record or structural variant is supported.
- Integrate only through a documented, versioned FAVOR API contract. Do not automate
  its upload form, send a 50 MB VCF to the hosted batch portal, scrape its result UI,
  or make the browser call FAVOR directly. The first contract is the public
  unauthenticated `POST /api/v1/variants/batch` endpoint with canonical
  `chromosome-position-REF-ALT` references and `depth=standard`. Its published
  contract accepts at most 10,000 references and returns results in input order;
  AnnoCAT still applies its lower product cap and response-size bounds.
- Use the same reviewed, unauthenticated API boundary for optional tissue context.
  The current candidate-summary contract is
  `POST /api/v1/variants/batch/enrichment`; explicit per-variant detail may use
  `/api/v1/variants/{reference}/enrichment`, `/qtls`, `/chrombpnet`,
  `/target-genes`, `/tissue-scores`, and `/region-overlaps`. Keep response schemas,
  timeouts, limits and endpoint availability versioned in the Rust provider client;
  the browser never calls these endpoints directly.
- Unauthenticated access is a hard requirement. Do not add login, OAuth, API-key,
  bearer-token, tenant, workspace or cookie settings for FAVOR. Use a fresh HTTP
  client without a cookie jar and send no browser session state. The anonymous batch
  contract was exercised successfully on 2026-07-21 with a two-variant request and
  no credentials. If a future schema advertises authentication or an anonymous call
  returns `401`/`403`, mark FAVOR unavailable with one actionable explanation; never
  ask the user to sign in or fall back to a workspace endpoint.
- Use `depth=standard` for the first release. It provides the compact cross-source
  fields needed for triage while keeping response size bounded. Do not request the
  full `detailed` payload for every row until response size, latency, field mapping
  and a deliberate on-demand detail interaction are measured. `minimal` may be used
  only for capability tests or previews, not silently substituted for the promised
  standard annotations.
- Send only assembly plus normalized chromosome, position, REF and ALT for eligible
  alleles. Never send the source VCF, sample names, genotypes/FORMAT, variant IDs,
  INFO values, report name, file path, case notes, phenotype profile, candidates or
  other variants. Explain that allele coordinates can themselves be sensitive and
  that FAVOR can observe the submitted alleles, IP address and request timing.
- Record FAVOR release/API contract, query time, requested field groups, returned
  upstream source/version metadata, warnings and unsupported alleles. Include FAVOR's
  required citation, copyright and terms metadata in About/export provenance; do not
  turn a provider annotation into a clinical classification.
- Keep Ensembl VEP REST as a possible later transcript-consequence provider only if
  FAVOR's transcript detail proves insufficient. Do not call both by default or add a
  generic provider abstraction before a second real provider is implemented.

**Variant Details tissue-context flow**

- Reuse the existing Variant Details pane and evidence order on any confirmed GRCh38
  report, whether VCF-only, locally annotated or imported with compatible provenance.
  Place one **Tissue & regulatory context** `Collapsible` after variant-level key
  evidence and optional
  phenotype relevance, and before **Transcript & molecular effect**. Do not add a
  FAVOR route, second inspector, nested dialog, or second copy of the variant summary.
- Before data exists, show one concise sentence and one secondary **Add tissue
  context** `Button`. Do not use `EmptyState` for this optional subsection. Opening a
  report, selecting a row, expanding the section, changing transcripts, sorting or
  filtering must never make an online request.
- The button opens the same privacy/GRCh38 confirmation pattern used by report-level
  enrichment, scoped to the current allele. After Tasks accepts the operation,
  replace it with a `StatusDot` paired with visible task text and a **View task**
  `Link`; do not put a second `ProgressBar` in Variant Details.
- After completion, lead with a compact `MetadataList columns="multi"`: strongest linked
  gene, strongest supported tissue, evidence types and bounded counts. A small
  `TabList size="sm" layout="fill" hasDivider` may then switch among **Overview**,
  **QTLs**, and **Regulation** because
  these are peer evidence views, not workflow steps. Use `TabMenu` only if those
  labels cannot fit at a supported narrow width.
- Use compact `List` rows rather than a wide `Table` inside the inspector. QTL rows
  show tissue, QTL type, linked gene, direction/effect and reported significance.
  Regulation rows summarize ChromBPNet, enhancer-gene, cCRE/region overlap and other
  returned evidence with source/version tooltips. Bound and rank the displayed rows;
  use `Badge` only for numeric counts and `Link` only for real navigation.
- Keep stage-one standard FAVOR annotations in the ordinary source-qualified field
  registry. Tissue detail remains inspector-first; only compact fields such as top
  linked gene or top tissue may be exposed through Columns, disabled by default.
- On the Candidates tab, place **Add tissue context to candidates** in its existing
  secondary `MoreMenu`. It uses the same provider client, authoritative candidate
  query, cap, task type and sidecar writer. Candidate summaries use the batch
  enrichment endpoint; detailed per-variant endpoints remain on demand for the
  opened variant. Do not implement a second batch engine.

**Storage, query and task behavior**

- Keep the immutable VCF-derived Parquet unchanged. Materialize bounded standard and
  tissue-context FAVOR data atomically under the existing report-sidecar contract,
  keyed by canonical assembly/allele identity, and join them through the existing
  DuckDB query path. This extends one sidecar/query mechanism; it does not introduce
  a FAVOR-specific report loader. Reopening, filtering, sorting, paging and exports
  then work without another network call.
- Add returned fields to the existing semantic field registry under a visible FAVOR
  source group, with human-readable descriptions, value direction and upstream
  source/version. Do not silently overwrite a local gnomAD, ClinVar, CADD, dbNSFP or
  other field with a similarly named FAVOR-delivered value.
- Re-running either enrichment stage requests only missing or explicitly refreshed
  alleles. Publish completed batches atomically, retain per-allele unsupported/error
  state, and use bounded retry/backoff without discarding already verified results.
- Respect the provider's `X-RateLimit-Limit`, `X-RateLimit-Remaining`,
  `X-RateLimit-Reset` and `Retry-After` headers. A `429` pauses the task until the
  advertised retry time; it does not spin, fail every allele, or ask for credentials.
- Run both operation types through the existing Tasks owner and task event format.
  Results and Variant Details may show a compact progress link, but they do not own
  another progress tracker. Cancellation leaves the base report and any previously
  complete enrichment usable and never converts partial remote output into an
  apparently complete annotation.
- Sharing can optionally include complete standard and tissue-context FAVOR sidecars
  and their provenance. Imported data is displayed through the same field/detail
  components and never silently refreshed or submitted again.

This flow is deliberately unsuitable for accidental whole-WGS upload. Full local
annotation remains the default for comprehensive WGS work; FAVOR enrichment is an
explicit shortcut for a user-reviewed, bounded shortlist or a small VCF. FAVOR is an
optional online service, not a downloadable Data Sources catalog item and not part of
the Minimal or Comprehensive local profile.

Do not add STAAR, MetaSTAAR, burden, SKAT, ACAT-V or cohort-association controls to
this phase. Those workflows require cohort genotypes, phenotypes and covariates, are
not exposed by the public anonymous variant-annotation API, and do not belong in an
individual variant-review inspector.

#### Phase 10F — Gene Context in two increments

First implement a patient-variant Gene Context route using the existing variants
endpoint with a gene rule, paging, sorting, filters and candidate state. Reuse the
Results query path and table; do not create a second gene-specific variant engine.

Add the exon/transcript map only after the first route proves useful. The compact
versioned transcript-layout sidecar should be emitted by the fastVEP/Ensembl core
builder or derived from its authoritative transcript artifact, not by an additional
AnnoCAT parser that duplicates the full GFF3 work. Astryx supplies the route frame,
`Selector`, metadata and table; the exon track is a focused AnnoCAT domain
component.

Initially show ClinVar evidence already attached to patient variants. Defer a nearby
reference-ClinVar regional index until its value, storage and query design are
measured; do not retain another full ClinVar copy just for this view.

#### Phase 10G — Gene-disease relationships

Implement GenCC first as a small, versioned gene-level resource queried by stable
gene identity. It is not a per-variant fastVEP annotation source and should not be
forced through the supplementary OSA adapter path. Ingest the current versioned
SGC ID/version download format, retain contributing organization/date/classification
and display contradictory assertions side by side with attribution.

ClinGen dosage/gene-validity data, OMIM and literature aggregation remain separate
later adapters after licensing, distribution, update and storage behavior are
verified. OMIM remains user-supplied/licensed.

#### Phase 10H — Explicitly deferred analytical scope

Do not include the following in the first clinical-review release:

- automatic ACMG classification, a combined pathogenicity score, diagnostic claims
  or automatic candidate creation;
- de novo, segregation or confirmed compound-heterozygous claims;
- any required online phenotype, FAVOR or gene.iobio service; FAVOR, Phen2Gene and
  Monarch remain explicit optional enrichments and every report remains locally
  viewable when they are unavailable;
- broad literature ingestion or automatic functional-evidence strength;
- family/pedigree setup, BAM/CRAM inspection or manual ACMG sign-off/reporting;
- FAVOR/STAAR cohort association, MetaSTAAR, burden, SKAT or ACAT-V analysis; these
  require a separate cohort-data, phenotype/covariate and statistical-validation
  design and are not implied by FAVOR variant annotation or tissue context;
- dominant-, recessive-, X-linked-, mitochondrial- and possible-biallelic workflows
  until sample, gene-disease, phase/sex and missing-evidence contracts are proven.

When later implemented, dominant/recessive/X-linked/mitochondrial options populate
visible explainable rules. Possible biallelic pairs is a separate bounded
gene-grouping analysis that shows both alleles and phase known/unknown; it is not an
ordinary PowerSearch filter.

## 7. Parity acceptance matrix

| Capability | Required fixture | Acceptance condition |
|---|---|---|
| Open report | Local and imported ZIP | Correct name, assembly, count, field catalog, and source metadata |
| Continuous loading | 1,000+ and chromosome 22 | Scrolls beyond first page; no duplicate, fixed ceiling, or blank reset |
| Row window | 10,000-row and WGS-scale generated fixture | DOM rows remain bounded with overscan/page eviction; scroll position, selection and candidates remain stable |
| Keyword search | Multiple core/evidence matches | Compact search returns the same complete-report result as production; stale searches cannot win |
| Structured search | Mixed PowerSearch clauses | Draft/Apply/Cancel/Reset are deterministic and compose with keyword search without filtering only loaded rows |
| Numeric filters | AlphaMissense, CADD, AF | Correct `=`, `!=`, `>`, `>=`, `<`, `<=`; missing is not zero |
| Gene list | Mixed comma/space/case input | Normalized expected gene set; compatible export format |
| Saved filters | Two reports with different schemas | Stable fields map; unavailable fields are explained, not silently dropped |
| Multi-sort | Core plus evidence fields | Correct full-result order, priority indicators, and natural reset |
| Columns | Dynamic unknown source | Show/hide/group/resize/reorder persist without losing unknown fields |
| Select all filtered | More matches than one page | All matching rows selected; individual exclusions work |
| Candidates | Local/reopened/Candidates tab | Add/remove persists and count/table agree |
| Exports | Explicit and filtered selection | Exact rows, visible columns, names, exclusions, and Save As output |
| Details | Multi-transcript golden variant | Correct scope switching, no flicker, complete evidence and provenance |
| Sample calls | Single/multi/multiallelic | Correct sample, genotype, selected-ALT depth, and missing state |
| Compact detail summary | FORMAT-rich, ClinVar/gnomAD-rich and sparse variants at normal/high zoom | Two-column summary retains sample call, population frequency, ClinVar, QUAL/FILTER, conservation and prediction agreement; it becomes one readable column without clipping or horizontal scrolling |
| Source completeness | Current and legacy reports with selected, unmatched and unretained fields | Every absence maps to the correct machine state; the UI never guesses from an empty value or repeats generic missing prose |
| Share/import | Existing and new package | Canonical data remains valid; supported review state round-trips |
| Responsive | 1024, 1366, 1920 px and 200% zoom | No hidden details, clipped actions, overlap, or unusable table |
| Tasks progress | Download, retained-part replay, validation, cache build, annotation, paused, reconnecting, failed and completed fixtures with two to four simultaneous tasks | Each active long-running task has exactly one stable progress bar; known totals show the authoritative percentage and coherent decimal units/rate/ETA, unknown totals remain visibly indeterminate, pause/resume and reconnect preserve task identity/value, phase changes do not regress completed work, controls remain available, and polling produces neither duplicate rows nor visible flicker |
| Profile installation | Fresh, partial, installed and active-source states | Profile contents, field choices, core dependency order, modes and concurrency match production; all eligible selections queue and active sources do not block the request |
| Cross-page flow | First-run, missing-source annotation, import and completed run | Return targets preserve transient work while every task/source/report view reloads authoritative server state |
| Portable review gate | Legacy/current shared packages | Candidates and included review state round-trip atomically; global presets and private notes are not silently copied |
| Phenotype gate | Local versioned HPO fixture plus mocked Phen2Gene and Monarch contracts | Human-readable multi-term selection and deterministic local ranking work offline; no network call occurs before explicit consent; the online response preserves provider-specific rank/evidence and provenance; PowerSearch rank, match, condition, scope and provider rules return the expected full-report rows; a saved rank rule without a profile reports that dependency; malformed, oversized, unavailable and cancelled responses fail safely; clearing the ranked-gene constraint restores the same All variants result |
| VCF-first report gate | GRCh38 single/multisample, multiallelic and dynamic-INFO fixtures plus declared GRCh37 and unknown-build fixtures; verified Core-ready and Core-unavailable states | **Open VCF for review** reuses New Annotation and opens a filterable `vcf-only` Parquet report without fastVEP or installed sources; when selected and Core is ready, the same run contract invokes provider-free fastVEP and produces `core-consequences` with real gene, transcript, consequence, HGVS and protein-effect fields; the option is disabled but ordinary review remains usable when Core is missing; allele rows retain correct source/ALT identity, local genotype evidence and dynamic INFO fields; unavailable biological fields are explained rather than fabricated; declared non-GRCh38 input is rejected and unknown assembly requires explicit GRCh38 confirmation without an unsupported build selector; both modes create exactly one existing Tasks entry and completed Results item |
| FAVOR enrichment gate | Mocked current FAVOR standard, batch-enrichment and per-variant tissue contracts with supported, unsupported, duplicate, partial, oversized, rate-limited, authentication-required and cancelled batches plus an opt-in live smoke test | The public calls succeed from a fresh cookie-free client without credentials; no call occurs when a report opens, a row is selected, a transcript changes, a filter/sort changes, or a tissue section expands; only confirmed normalized allele identities leave after the form-dialog consent action; selected/filtered and candidate scopes use authoritative counts, the provider/app cap and the existing task owner; no sample, genotype, INFO, name, note, phenotype, candidate state or path is transmitted; standard fields join through the semantic field registry and tissue context renders in the existing details hierarchy; retries preserve verified batches and honor rate-limit headers; `401`/`403` disables the provider instead of starting authentication; cancel/offline/provider failure leaves the base report usable; reopen/share/import never triggers an implicit request |
| Theme gate | Generated AnnoCAT light/dark theme at desktop, narrow and 200% zoom fixtures | One SideNav control changes and persists appearance without reload; both modes preserve semantic colors, contrast, table boundaries, focus indicators and clinical meaning |
| Gene Context gate | Multi-exon plus strand fixtures | Patient variants reuse the canonical query path; transcript layout matches the recorded Ensembl release and never loads all WGS rows |
| Gene-disease gate | Versioned GenCC fixture with conflicting assertions | Stable gene matching, source attribution and contradictory claims remain distinct; no assertion is converted into a variant classification |
| Accessibility | Keyboard and screen-reader smoke | All primary actions operable and announced without mouse/color dependence |
| Performance | 10,000-row and chr22 fixtures | No meaningful regression from recorded production baseline without explanation |

## 8. Rollback strategy

- Make each phase a separate local commit after its acceptance gate passes.
- Do not combine API contract changes, visual restructuring, and deletion of the old screen in one commit.
- Keep the existing Results screen selectable during development.
- If a new route fails, return to the old route without migrating or rewriting user data.
- Never use report, settings, or cache migration as a prerequisite for rolling back a visual build.
- Backward-compatible API additions land before the React consumer; obsolete response fields are removed only after the old consumer is deleted.
- Any review-state migration remains atomic and independently reversible according to the clinical plan.

## 9. Code-size and duplication controls

At the end of each phase, review:

- Which old functions and CSS rules are now obsolete?
- Did React add a second formatter, filter compiler, selection tracker, or task-state model?
- Can a new wrapper be deleted by calling the existing API directly?
- Is state stored in more than one place?
- Is a component generic because two real consumers use it, or merely speculative?
- Did any fixed field list replace dynamic report metadata?
- Are mock data and production data paths still mixed?

The cutover is not complete if the new UI works but the old implementation remains indefinitely. The final migration should produce one coherent application, not a React application layered on top of the existing one.

## 10. Immediate next actions

1. Pin Astryx stable `0.1.7`, Lucide and the package manager in the lockfile; record third-party notice requirements.
2. Preserve the parallel SPA and stop adding mock-only behavior.
3. Implement the complete Phase 0 page and flow baseline.
4. Configure typed hash routes and the Vite `/api` proxy to the real service and data roots.
5. Replace the 84 synthetic rows with the typed, cancellable real-results client and bounded row window.
6. Restore dynamic columns, both search modes, filters, resizing, reordering, persistence and controlled server sorting.
7. Implement the remaining pages—including the redesigned profile dialog—in the parallel SPA, then perform one final application cutover.
