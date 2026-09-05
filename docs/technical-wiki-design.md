# AnnoCAT technical wiki design and maintenance workflow

## Document status

| Item | Value |
| --- | --- |
| Status | Proposed design pending approval |
| Scope | Documentation structure, authoring, verification, and publication |
| Authoring model | Manual, on-demand Codex task with human review |
| Publication model | Deterministic GitHub Pages deployment after an approved merge |
| Canonical content | Markdown committed in the AnnoCAT repository |
| Last reviewed | 2026-09-04 |

This document defines the complete redesign of AnnoCAT's documentation into a
technical wiki with the navigability and source-linked explanations associated
with DeepWiki. It does not authorize changes to application behavior, annotation
behavior, result schemas, scientific methods, or the fastVEP pin.

## Decision

AnnoCAT will maintain one canonical technical wiki in this repository and
publish it with Material for MkDocs on GitHub Pages.

Documentation content will not be generated on a schedule or rewritten after
every code commit. A maintainer will start a Codex task when documentation needs
to be created or refreshed. Codex will inspect the relevant source, tests,
manifests, commits, and primary references; propose Markdown changes; run the
documentation checks; and leave the changes for human review. Merging approved
Markdown will trigger a deterministic site build and deployment.

The current documentation is source material for the rewrite, not an authority
that must be preserved verbatim. Correct technical and scientific content will
be retained, reorganized, shortened where possible, and split into pages that
answer one coherent question.

The AnnoCAT wiki will include the parts of the maintained fastVEP fork that
define AnnoCAT's behavior. It will link to the fork's source and technical wiki
for internal details instead of copying all standalone fastVEP documentation.

## Desired outcome

The finished site must let a reader answer these questions without reading the
repository from top to bottom:

1. What does AnnoCAT do, and what does it deliberately not do?
2. How does an input variant become an annotated result row?
3. Which component owns each annotation, transformation, and display decision?
4. How are variants, alleles, transcripts, genes, phenotypes, conditions, and
   pathways represented?
5. Which behavior is implemented, proposed, experimental, deprecated, or
   historical?
6. What evidence supports a scientific or correctness claim?
7. Which fastVEP fork revision is used, why is a fork required, and how is it
   validated against upstream and Ensembl VEP?
8. How can a maintainer reproduce a build, test, validation, or release?

The presentation should provide:

- hierarchical navigation on the left;
- a page table of contents on the right;
- client-side full-text search;
- stable headings and deep links;
- architecture, data-flow, sequence, and state diagrams where they clarify a
  relationship;
- concise source maps linking explanations to repository files and tests;
- related-page navigation; and
- readable behavior on desktop, tablet, mobile, keyboard, and screen readers.

Material for MkDocs provides the required navigation, table-of-contents,
search, code, and diagram features without a documentation server or database.
Its search runs in the browser. GitHub Pages can deploy the built static
artifact through GitHub Actions.

## Explicit non-goals

The initial wiki will not include:

- an AI question-and-answer interface;
- automatic AI authoring or automatic AI merges;
- a documentation database, search service, or application server;
- duplicated copies of every page from the fastVEP fork;
- documentation for standalone fastVEP interfaces that AnnoCAT does not use;
- documentation versioning with `mike` or another version manager;
- analytics, comments, user accounts, or telemetry;
- a blog, changelog generator, or release-note generator;
- custom MkDocs plugins when a built-in feature is sufficient; or
- a promise that a documented score or annotation constitutes clinical advice.

Git history and release tags are sufficient for retrieving older documentation
until users demonstrate a need to browse multiple released versions at once.

## Audience

The wiki serves four overlapping audiences:

| Audience | Primary needs |
| --- | --- |
| AnnoCAT users | Installation, workflows, result interpretation, limitations, privacy, and troubleshooting |
| Developers | Architecture, component ownership, APIs, schemas, tests, and local development |
| Scientific reviewers | Methods, data provenance, validation design, benchmark definitions, and limitations |
| Release maintainers | Source versions, fastVEP pinning, build reproducibility, validation gates, and packaging |

A page should identify its audience through its location and opening paragraph,
not through separate duplicated versions of the same explanation.

## DeepWiki-inspired feature boundary

The objective is to reproduce the useful documentation experience, not to clone
DeepWiki as a product.

| Capability | Initial AnnoCAT wiki | Reason |
| --- | --- | --- |
| Hierarchical topic navigation | Yes | Core discoverability requirement |
| Page table of contents | Yes | Makes long technical pages usable |
| Full-text search | Yes | Built into Material for MkDocs |
| Architecture and data-flow diagrams | Yes | Explains cross-component behavior |
| Source and test links | Yes | Makes claims reviewable |
| Related pages | Yes | Preserves context without duplicating prose |
| Repository and revision identity | Yes | Required for reproducibility |
| AI-authored first draft | Manual task | Useful when grounded in the repository |
| Continuous repository indexing | No | Unnecessary for the current maintenance cadence |
| AI chat over the wiki | No | Adds cost, privacy, retrieval, and accuracy obligations |
| Automatic publication of approved Markdown | Yes | Deterministic and low risk |

## Authority and evidence model

Documentation must not become a second implementation specification that can
silently disagree with the product. Claims are evaluated in this order:

1. **Committed implementation at the documented revision.** Source defines
   runtime behavior.
2. **Executable tests and retained validation artifacts.** These establish
   which behavior has been exercised and what passed.
3. **Versioned manifests and schemas.** Examples include
   [`config/fastvep-pin.json`](../config/fastvep-pin.json), source catalogs,
   validation manifests, and result schemas.
4. **Official specifications and primary scientific sources.** These support
   intended meaning and scientific rationale but do not prove that AnnoCAT
   implements the method correctly.
5. **Current AnnoCAT prose.** Existing documents are leads that must be checked
   against items 1 through 4.
6. **AnnoCAT and fastVEP DeepWiki pages.** These are discovery and organization
   aids, never release or behavior authorities.

When two sources disagree, the page must either describe the implemented
behavior and record the discrepancy or state that the claim is unresolved. It
must not silently combine them into a plausible narrative.

### Claim classes

Each material claim should be supportable as one of these classes:

| Claim | Required support |
| --- | --- |
| Runtime behavior | Source path or symbol plus an applicable test |
| Public UI behavior | UI source plus a browser or component test when available |
| File or API contract | Versioned schema, parser, serializer, and compatibility test |
| Dataset identity | Provider, release/version, retrieval location, checksum or manifest, and assembly where applicable |
| Scientific method | Primary method paper or official ontology specification plus implementation tests |
| Validation result | Immutable inputs, tool and data versions, command/configuration, expected metric, observed metric, and retained artifact |
| Performance result | Hardware, operating system, dataset, revisions, command, repetitions, statistic, and output-equivalence check |
| Product comparison | Public product documentation or published literature, with inference clearly marked |

### Status vocabulary

Pages must use these terms consistently:

- **Implemented:** present in the documented AnnoCAT revision.
- **Proposed:** a reviewed design that is not yet implemented completely.
- **Planned:** an accepted but not yet implemented maintenance action, used in
  decision ledgers such as upstream tracking.
- **Experimental:** implemented but not part of a stable compatibility promise.
- **Deprecated:** still present but scheduled for removal.
- **Historical:** retained only to explain an earlier decision or migration.
- **Unresolved:** evidence is insufficient or contradictory.

Words such as “supported,” “validated,” “equivalent,” “correct,” and “compatible”
must name their scope. For example, byte-identical output on one fixture is not
general equivalence with Ensembl VEP.

## Canonical repository layout

The initial implementation should use the smallest conventional layout:

```text
AnnoCAT/
├── mkdocs.yml
├── requirements-docs.txt
├── docs/
│   ├── index.md
│   ├── getting-started/
│   ├── workflows/
│   ├── architecture/
│   ├── annotation/
│   │   └── fastvep/
│   ├── results/
│   ├── phenotype/
│   ├── validation/
│   ├── reference/
│   ├── development/
│   └── assets/
└── .github/
    └── workflows/
        └── documentation.yml
```

No separate documentation repository, Git submodule, content database, or
generated source tree is required. Keeping the wiki beside the code makes a
documentation diff reviewable with the implementation diff.

`requirements-docs.txt` must pin an exact reviewed Material for MkDocs version
and its required transitive versions so local and CI builds are reproducible.
The version is selected during implementation and updated through an explicit
dependency change, not a floating install in the publishing workflow.

## Proposed information architecture

### Home

- What AnnoCAT is
- Supported inputs and outputs
- High-level workflow
- Current release and documented commit
- Important limitations
- Links for users, developers, scientific reviewers, and maintainers

### Getting started

- Installation and updates
- First annotation
- Opening and reopening results
- Local data and privacy
- Troubleshooting

### User workflows

- Annotate a VCF
- Add optional annotations
- Search, filter, and sort results
- Select transcripts and inspect evidence
- Filter with genes, HPO features, MONDO conditions, and Reactome pathways
- Save, reopen, and export results

### System architecture

- Component overview
- End-to-end annotation data flow
- Process and worker model
- Storage and temporary files
- Result schemas and compatibility
- Web application architecture
- Error handling and recovery

### Annotation pipeline

- Input parsing and normalization
- Transcript consequence annotation
- Supplementary annotation sources
- Structured evidence projection
- Missing values and failure semantics
- Annotation performance and memory model

### fastVEP annotation engine

- Why AnnoCAT maintains a fork
- AnnoCAT–fastVEP integration contract
- Fork change categories
- Consequence and HGVS behavior
- Transcript-cache behavior
- OSA1, OSA2, and supplementary evidence
- Memory and performance changes
- Fork pinning and release packaging
- Upstream divergence and adoption decisions
- Ensembl VEP concordance and oracle validation
- Known limitations and unresolved divergences

### Results and evidence

- Result row and allele model
- Transcript selection
- Evidence selection and display
- Filtering and sorting
- Gene-list filtering and upstream/downstream consequences
- FAVOR annotations
- Caching and pagination

### Phenotype, condition, pathway, and gene resolution

- Unified search model
- HPO feature resolution
- MONDO condition resolution
- Reactome pathway resolution
- HGNC normalization
- Gene association categories
- Patient phenotype profile and aggregation
- Phenotype ranking and score interpretation
- UI interaction and accessibility contract
- Scientific validation and cohort benchmarking
- Dataset releases and provenance
- Deliberate limits and open decisions

### Validation and scientific correctness

- Validation strategy and claim levels
- Annotation concordance
- Supplementary-source parity
- Result projection
- Phenotype known cases and cohort evaluation
- Oracle selection and independence
- Performance benchmarking
- Release qualification
- Reproducibility records

### Reference

- Command-line interface
- Configuration
- APIs
- Result schemas
- Annotation field catalog
- Supported source and ontology versions
- Error messages
- Terminology

### Development and maintenance

- Local development
- Test suites
- Documentation maintenance
- Release process
- Updating sources and ontologies
- Updating the fastVEP pin
- Security and responsible disclosure

## Page design

Pages should generally follow this sequence, omitting sections that do not add
information:

1. **Summary:** what the component or workflow does.
2. **Product boundary:** what is and is not included.
3. **How it fits:** relationship to upstream and downstream components.
4. **Flow or behavior:** a diagram followed by an equivalent prose description.
5. **Contracts and invariants:** inputs, outputs, identities, ordering, missing
   values, failure behavior, and compatibility.
6. **Implementation map:** source files, important symbols, and tests.
7. **Verification:** what is tested and what a passing test establishes.
8. **Limitations:** known exclusions, uncertainty, and unresolved behavior.
9. **References:** primary scientific, standards, and official provider links.
10. **Related pages:** the next useful topics.

Pages should not begin with repository trivia. They should first explain the
user-visible or system-level concept, then lead readers to implementation
details.

### Page metadata

Each rewritten technical or scientific page should begin with concise YAML
front matter:

```yaml
---
title: HPO feature resolution
description: How AnnoCAT resolves HPO terms and constructs associated gene sets.
status: implemented
verified_commit: <full AnnoCAT commit SHA>
last_reviewed: YYYY-MM-DD
---
```

The `verified_commit` is the AnnoCAT revision inspected for the page. A page
that describes a pinned dependency must also identify that dependency revision
in its source map. Updating one page does not require falsely advancing the
verification revision of untouched pages.

No custom plugin is required to interpret this metadata initially. The page
must also show a short visible status note when its status is not
`implemented`.

### Source maps

Technical pages should end with a compact source map rather than scattering
line-number citations through every paragraph:

| Role | Source | Verification |
| --- | --- | --- |
| Request handling | `crates/.../main.rs`, named handler | Relevant integration test |
| Domain logic | `crates/.../phenotype.rs`, named type or function | Relevant unit and oracle tests |
| UI | `web/src/app/phenotypes.js`, named component or function | Browser or JavaScript test |

File and symbol links are required. Line links are optional because they become
stale quickly. The page-level revision identifies the version of the source
that was reviewed.

### Diagrams

Use Mermaid only when a relationship is materially easier to understand as a
diagram. Appropriate uses include:

- end-to-end data flows;
- process and worker boundaries;
- request sequences;
- schema or cache lifecycles;
- component dependencies; and
- validation gates.

Every diagram must have a short prose equivalent for accessibility and for
readers viewing raw Markdown. Avoid decorative diagrams and diagrams that
repeat a three-item list.

Material for MkDocs supports Mermaid through the existing
`pymdownx.superfences` extension configuration, so no separate Mermaid plugin
is needed.

## fastVEP fork documentation boundary

### Repositories and current identity

At this document's review date:

| Item | Identity |
| --- | --- |
| Upstream | [Huang-lab/fastVEP](https://github.com/Huang-lab/fastVEP) |
| Maintained fork | [annocat-project/fastVEP](https://github.com/annocat-project/fastVEP) |
| Fork technical wiki | [DeepWiki for annocat-project/fastVEP](https://deepwiki.com/annocat-project/fastVEP) |
| AnnoCAT pin | `78c870be81762f8cec0a020a76a0515cfdd1449c` |
| Upstream base | fastVEP 0.3.0 at `0e13c5b` |
| Authority | [`config/fastvep-pin.json`](../config/fastvep-pin.json) |

These identities are current-state context, not duplicated configuration. If
the manifest changes, the wiki page describing the current pin must be reviewed
and updated in the same change.

### What belongs in the AnnoCAT wiki

The AnnoCAT wiki owns:

- why the fork is required by AnnoCAT;
- the exact commit and upstream base used by a release;
- how packaging obtains, verifies, builds, and bundles `fastvep`;
- the exact commands and options AnnoCAT invokes;
- the input, output, cache, and structured-evidence contracts AnnoCAT consumes;
- fork behavior on which AnnoCAT results depend;
- compatibility effects of a fork update;
- validation against Ensembl VEP, source-native oracles, and AnnoCAT result
  projection;
- performance and memory measurements made under AnnoCAT's workload; and
- known divergences, limitations, and update decisions.

### What remains fork-specific

The fastVEP repository and its own wiki remain authoritative for:

- the complete internal crate architecture;
- standalone CLI and web-server interfaces;
- source builders not distributed or invoked by AnnoCAT;
- the experimental fastVEP ACMG classifier;
- general-purpose multi-organism behavior;
- internal binary-format implementation beyond AnnoCAT's compatibility needs;
  and
- upstream-facing development instructions.

An AnnoCAT page may summarize a fork implementation detail when it explains an
observable contract, correctness decision, performance property, or release
gate. It must link to the exact pinned source and must not imply that unused
standalone functionality is exposed by AnnoCAT.

### Required fastVEP pages

#### Why the fork exists

Group maintained changes by product purpose rather than reproducing the ordered
commit array from `fastvep-pin.json`. The categories currently include:

- streaming and bounded-memory source builders;
- strict OSA1/OSA2 validation and lossless record handling;
- deterministic supplementary-source loading and lookup;
- structured evidence output and projection efficiency;
- transcript-cache integrity and read-only runtime safeguards;
- consequence and HGVS corrections;
- source-specific field and missing-value preservation; and
- privacy-preserving performance diagnostics.

The manifest remains the only exhaustive ordered list of fork commits.

#### Integration contract

Document:

- binary identity and verification;
- process invocation and whether an option is explicit or inherited from the
  pinned default;
- required gene model, reference, transcript cache, and supplementary caches;
- structured output consumed by AnnoCAT;
- allele, transcript, gene, field, and missing-value semantics;
- warning and failure propagation; and
- temporary and requested output differences.

#### Correctness and validation

Document the distinction between:

- fork unit and integration tests;
- source-cache structural and semantic validation;
- OSA1/OSA2 parity;
- direct-GFF and transcript-cache parity;
- targeted Ensembl VEP concordance;
- broader differential and metamorphic testing;
- GIAB end-to-end smoke and performance runs; and
- unresolved differences requiring an independent oracle.

A concordance percentage must always state the dataset, denominator,
comparison fields, tool versions, configuration, and treatment of multi-valued
consequences. Agreement with another program is evidence of concordance, not by
itself proof of biological correctness.

#### Upstream tracking

The wiki should provide a concise current summary and link to the detailed
decision ledger. Each upstream decision retains the ledger vocabulary already
used by AnnoCAT: implemented, superseded, partial, planned, deferred, or
skipped. Use unresolved when the available evidence does not yet support a
decision. The page must distinguish a cherry-pick from an independently ported
behavior.

## Manual documentation workflow

### Trigger

A maintainer starts the task explicitly. Suitable times are:

- before a release;
- after a user-visible workflow changes;
- after an API, schema, file format, or compatibility boundary changes;
- after an ontology, annotation source, or scientific method changes;
- after the fastVEP pin or invocation changes;
- after validation changes what AnnoCAT can accurately claim; or
- when a page is reported as missing, unclear, or incorrect.

There is no scheduled AI task and no AI task triggered by a code push.

### Inputs

The task requester supplies or lets Codex determine:

1. the documentation objective;
2. the AnnoCAT comparison range, preferably `<previous-release>..HEAD`;
3. the exact current AnnoCAT revision;
4. affected issues, pull requests, and validation artifacts;
5. the currently pinned fastVEP revision when annotation behavior is involved;
6. primary scientific or standards references when a scientific claim changes;
   and
7. whether the task is a focused page update or a release-wide audit.

No separate “last documented commit” database is required. Release tags, Git
history, and page metadata provide the necessary baseline.

### Required Codex procedure

Codex must:

1. Read the complete request and repository instructions.
2. Inspect the worktree and preserve unrelated changes.
3. Identify the affected public behavior and documentation pages.
4. Read the relevant implementation paths end to end, including callers and
   consumers when a contract crosses components.
5. Read the applicable tests, schemas, manifests, and retained validation
   artifacts.
6. For fork-dependent behavior, inspect the exact commit named by
   `config/fastvep-pin.json`, not merely the current fork branch or DeepWiki.
7. Verify scientific claims with primary sources and official provider
   documentation. Clearly label inferences.
8. Update only the pages needed for the objective.
9. Preserve the distinction between implemented behavior and design proposals.
10. Run the documentation build in strict mode.
11. Inspect the rendered pages at desktop and narrow widths when layout,
    diagrams, tables, or images changed.
12. Summarize changed pages, verification performed, and claims that remain
    unresolved.

Codex must not change application code during a documentation-only task unless
the user separately authorizes implementation.

### Reusable task prompt

```text
Review AnnoCAT changes from <BASE> to <HEAD> and update the technical wiki for
<SCOPE>.

Use committed source, tests, schemas, manifests, and retained validation
artifacts as the implementation authority. Treat existing prose and DeepWiki
pages as discovery aids that must be verified. If fastVEP behavior is involved,
inspect the exact revision in config/fastvep-pin.json and distinguish AnnoCAT's
integration from standalone fastVEP features.

Update only affected Markdown pages. Preserve the wiki information architecture,
terminology, cross-links, page status, verified commit, and source maps. Clearly
separate implemented, proposed, experimental, deprecated, historical, and
unresolved behavior. Support scientific claims with primary or official sources
and do not turn concordance into a broader correctness claim.

Do not change application code. Run the strict documentation build and inspect
the rendered pages affected by structural or visual changes. Before finishing,
report what changed, what was verified, and every material claim that could not
be proven.
```

### Human review

The reviewer inspects the Markdown and rendered result before merging. The
reviewer must check:

- whether the page answers the intended question;
- whether each material behavior claim matches the cited revision;
- whether status labels are accurate;
- whether a scientific source supports the exact claim made;
- whether validation scope and limitations are visible;
- whether fastVEP standalone behavior has been mistaken for AnnoCAT behavior;
- whether old information was removed intentionally rather than lost during a
  split;
- whether links, diagrams, tables, headings, and mobile layout work; and
- whether the change contains patient data, private paths, secrets, or internal
  release information that should not be public.

The same maintainer may request and approve a small update, but approval still
requires an explicit diff and rendered-page inspection rather than accepting
the AI summary alone.

### Commit and publication

After approval:

1. Commit the documentation changes on a normal branch.
2. Push and merge through the repository's usual review process.
3. The documentation workflow builds the exact merged Markdown.
4. The workflow uploads the static artifact to GitHub Pages.
5. Verify the published URL and one changed deep link.

The AI is not part of publication. It cannot alter content after review because
the deployment builds committed files only.

## Initial rewrite workflow

The first conversion is larger than routine maintenance and should proceed in
reviewable groups.

### Phase 1: inventory and claim audit

1. Inventory every current documentation page and heading.
2. Mark each section as retain, rewrite, split, merge, historical, or remove.
3. Identify the implementation, test, manifest, or primary reference supporting
   each material claim.
4. Record contradictions and unverified statements rather than resolving them
   by assumption.
5. Establish the target navigation and page ownership.

Deliverable: a migration map and unresolved-claim list. No old page is removed
in this phase.

### Phase 2: site frame

1. Add the pinned documentation dependency file.
2. Add a minimal `mkdocs.yml` using Material, built-in search, navigation, code,
   and Mermaid support.
3. Add the home page and top-level section landing pages.
4. Add the strict documentation build.
5. Check keyboard navigation, narrow layout, and search indexing with placeholder
   links to existing pages.

Deliverable: a functioning but not yet reorganized local wiki.

### Phase 3: architecture and fastVEP

Rewrite the pages that establish the system boundary first:

1. system overview and end-to-end flow;
2. annotation pipeline;
3. fastVEP fork purpose and integration contract;
4. pinning, packaging, and upstream tracking; and
5. annotation correctness and validation.

These pages establish terminology and source boundaries needed by later pages.

### Phase 4: results and phenotype systems

1. Rewrite result, allele, transcript, and evidence selection pages.
2. Split the phenotype and gene-resolution specification into focused pages.
3. Keep design-only ranking or validation proposals visibly separate from
   implemented user behavior.
4. Cross-link upstream/downstream gene filtering with fastVEP's consequence
   distance contract and transcript selection guidance.

### Phase 5: user and maintainer workflows

Rewrite installation, annotation, filtering, privacy, CLI, troubleshooting,
development, source-update, and release procedures. Examples must be run or
checked against the documented release.

### Phase 6: migration and publication

1. Verify that every retained fact from the migration map has a canonical home.
2. Replace moved legacy pages with short pointer pages when external links are
   likely to exist.
3. Exclude pointer pages from search so they do not compete with canonical
   content.
4. Run the complete documentation gate.
5. Publish to a preview or temporary Pages environment and inspect it.
6. Enable the canonical Pages URL after approval.

Do not delete a legacy page merely because a new navigation entry exists.
Removal follows verified migration of its supported content and inbound links.

## Migration map for current documents

| Current document | Target treatment |
| --- | --- |
| `docs/README.md` | Replace with the wiki home page at `docs/index.md`; update repository links |
| `docs/installation.md` | Rewrite under Getting started |
| `docs/annotation.md` | Split between user workflow and annotation pipeline |
| `docs/results.md` | Split between user workflow and result model |
| `docs/filtering.md` | Rewrite under User workflows and cross-link the query engine |
| `docs/cli.md` | Retain as reference; verify every example against current help |
| `docs/data-and-privacy.md` | Retain and expand only from verified storage behavior |
| `docs/transcript-and-evidence-selection.md` | Split into transcript selection, evidence selection, and gene-filter behavior |
| `docs/evidence-display.md` | Merge overlapping explanations into Results and evidence |
| `docs/phenotype-and-gene-resolution.md` | Split into search, HPO, MONDO, Reactome, HGNC, profile/ranking, UI, provenance, validation, and decision pages |
| `docs/annotation-validation.md` | Rewrite as the annotation-validation overview |
| `docs/github-actions-source-validation.md` | Split into source validation, release qualification, and reproducibility |
| `docs/fastvep-maintenance.md` | Rewrite as fork identity, integration, pinning, and update procedure |
| `docs/fastvep-upstream-tracking.md` | Retain as a detailed decision ledger with a shorter wiki summary |
| `docs/result-import-security.md` | Retain under architecture/security and verify against current schemas |

The inventory phase may adjust destinations, but it must not create two
authoritative pages for one contract.

## Minimal MkDocs design

The implementation should begin with Material's built-in capabilities. A
representative configuration is:

```yaml
site_name: AnnoCAT Technical Wiki
site_url: https://annocat-project.github.io/AnnoCAT/
repo_url: https://github.com/annocat-project/AnnoCAT
repo_name: annocat-project/AnnoCAT
strict: true

theme:
  name: material
  features:
    - navigation.sections
    - navigation.path
    - navigation.top
    - navigation.tracking
    - search.highlight
    - search.suggest
    - content.code.copy

plugins:
  - search

markdown_extensions:
  - admonition
  - attr_list
  - footnotes
  - md_in_html
  - tables
  - toc:
      permalink: true
  - pymdownx.details
  - pymdownx.highlight
  - pymdownx.inlinehilite
  - pymdownx.superfences:
      custom_fences:
        - name: mermaid
          class: mermaid
          format: !!python/name:pymdownx.superfences.fence_code_format
```

The final configuration must contain an explicit `nav` tree matching the
approved information architecture. Start without custom templates, JavaScript,
CSS, or additional plugins. Add styling only if the rendered default prevents
the required hierarchy, branding, accessibility, or readability.

## Local authoring commands

The implemented repository should document one supported setup and use the same
pinned requirements in local and CI environments:

```powershell
py -m venv .venv-docs
.\.venv-docs\Scripts\python.exe -m pip install -r requirements-docs.txt
.\.venv-docs\Scripts\python.exe -m mkdocs serve
.\.venv-docs\Scripts\python.exe -m mkdocs build --strict
```

The generated `site/` directory is a build artifact and must be ignored rather
than committed.

## GitHub Pages workflow

The publication workflow is deterministic automation, not automated writing.
It should run when documentation inputs change on the default branch and permit
a manual run for recovery.

Required behavior:

1. Check out the merged revision.
2. Install the pinned documentation requirements.
3. Run `mkdocs build --strict`.
4. Configure GitHub Pages.
5. Upload the generated static artifact.
6. Deploy through the protected `github-pages` environment.
7. Expose the deployed URL in the workflow summary.

Use GitHub's maintained `configure-pages`, `upload-pages-artifact`, and
`deploy-pages` actions with only the documented permissions:

- `contents: read` for the build;
- `pages: write` for deployment; and
- `id-token: write` for deployment authentication.

The workflow should watch only documentation inputs such as `docs/**`,
`mkdocs.yml`, `requirements-docs.txt`, and its own workflow file. A source-code
commit that does not update documentation must not republish the site merely to
create noise.

Pull requests should run the strict build without deploying. Deployment occurs
only from the repository's approved default branch. GitHub Pages environment
protection may be added if repository policy requires an additional publication
approval.

## Documentation verification gate

### Required for every documentation pull request

- Pinned dependencies install successfully.
- `mkdocs build --strict` succeeds.
- Every configured navigation target exists.
- Internal links and anchors referenced by changed pages resolve.
- Images have useful alternate text.
- Mermaid diagrams parse and include prose equivalents.
- No duplicate top-level heading or broken heading hierarchy is introduced.
- Code and command examples changed by the task were run, mechanically checked,
  or explicitly marked illustrative.
- Page status, verified revision, and last-reviewed date are present where
  required.
- The diff contains no generated `site/` content, secrets, patient data, or
  machine-specific private paths.

### Required for structural or visual changes

Render and inspect:

- the home page;
- one short page;
- one long technical page;
- one page containing a wide table;
- one page containing a diagram;
- navigation and search at desktop width; and
- navigation, tables, code, and headings at a narrow mobile width.

Keyboard focus, search operation, skip/navigation behavior, and visible focus
must remain usable. Do not use color alone to communicate status.

### Required for scientific content changes

- Confirm that references are primary literature, an official ontology or data
  provider, or an applicable standard whenever available.
- Record the ontology, annotation, tool, and assembly versions relevant to the
  claim.
- Separate association evidence from causal or clinical interpretation.
- Separate method validity from implementation correctness.
- Describe negative, missing, filtered, and unsupported cases.
- State benchmark cohort construction, leakage controls, metrics, confidence
  intervals where applicable, and limitations.
- Ensure that an oracle is sufficiently independent of the implementation under
  test.
- Preserve uncertainty instead of presenting an unresolved discrepancy as a
  settled result.

### Required for fastVEP content changes

- Read the exact manifest pin and verify the linked commit exists.
- Confirm the packaged binary identity when discussing a release.
- Distinguish inherited upstream behavior, cherry-picked behavior, independently
  ported behavior, fork-only behavior, and deferred behavior.
- Confirm whether an invocation option is explicit or inherited from the pinned
  default.
- Check compatibility effects on OSA caches, transcript caches, result schemas,
  and previously saved results.
- Link validation claims to exact fixtures, commands, artifacts, and revisions.
- Avoid claiming biological superiority from disagreement with Ensembl VEP
  unless an independent specification or experimental oracle supports it.

## Scientific communication rules

AnnoCAT combines technical implementation with biomedical knowledge. The wiki
must maintain these distinctions:

- A phenotype–gene association is not proof that a variant is causal.
- A semantic-similarity rank is prioritization evidence, not a diagnostic
  probability unless a validated probabilistic model explicitly establishes
  that interpretation.
- A nearby upstream or downstream variant is not shown to regulate a selected
  gene by proximity alone.
- A computational prediction is not equivalent to clinical classification.
- An automated ACMG implementation is not exposed merely because the pinned
  annotation engine contains related standalone code.
- Agreement with an external tool measures concordance under the tested
  configuration; it does not prove universal correctness.
- Dataset provenance and release identity are part of the scientific method,
  not installation trivia.

User-facing pages should explain these limits in plain language. Technical pages
should additionally explain the exact data and algorithmic boundaries.

## Accessibility and writing standards

- Use one level-one heading per page and do not skip heading levels.
- Use descriptive link text instead of “click here.”
- Introduce acronyms on first use unless they are part of the product name.
- Keep paragraphs focused and prefer tables only for genuine comparisons or
  repeated fields.
- Give images meaningful alt text; decorative images should be avoided.
- Provide prose equivalents for diagrams and do not encode distinctions by
  color alone.
- Keep commands copyable and state their working directory and prerequisites.
- Identify placeholders visibly so users do not paste them unchanged.
- Separate instructions from explanation and state destructive effects before
  the command that causes them.
- Do not use screenshots as the sole documentation of a workflow.
- Avoid language that overstates clinical, scientific, security, or performance
  assurance.

## Privacy and security

The public wiki and its build artifacts must not contain:

- patient variants, phenotypes, identifiers, or result files;
- access tokens, API keys, cookies, or signed download URLs;
- private local paths or usernames;
- unpublished vulnerability details;
- licensed dataset contents that cannot be redistributed; or
- internal release credentials or infrastructure configuration.

Examples must use synthetic or explicitly redistributable fixtures. Do not add
analytics, external comments, or third-party search in the initial site. The
built-in client-side search is sufficient and avoids sending search terms to a
service.

## Staleness and maintenance policy

Documentation review is required in the same change when any of these contracts
change:

- supported input or output;
- user-visible workflow or terminology;
- CLI argument or default;
- API route or schema;
- result schema or reopening compatibility;
- annotation field, source, missing-value rule, or provenance;
- ontology or association dataset;
- phenotype ranking method or display;
- fastVEP pin, invocation, cache contract, or structured output;
- validation fixture, oracle, metric, threshold, or public claim; or
- release packaging and platform support.

Code review may request a documentation change, but no automated AI task is
needed. Before a release, run the manual Codex task across the previous release
tag and release candidate even if individual changes claimed not to require
documentation.

Pages that cannot be reverified should retain their earlier verified revision
and receive an explicit stale or unresolved note if the relevant implementation
has changed.

## Acceptance criteria

The redesign is complete when all of the following are true:

1. A reader can navigate from the home page to every canonical topic without
   knowing a filename.
2. Search returns useful results for product terms, source names, HPO/MONDO IDs,
   commands, schemas, and error concepts represented in the docs.
3. The end-to-end architecture and annotation flow are documented with source
   maps and accessible diagrams.
4. The maintained fastVEP fork has the complete AnnoCAT-facing coverage defined
   in this document.
5. Standalone fastVEP features are not presented as AnnoCAT interfaces.
6. The large phenotype specification has been divided into navigable pages
   without mixing planned ranking work with implemented search behavior.
7. Every material scientific and validation claim has suitable evidence and a
   stated scope.
8. Existing supported documentation content has a canonical destination or an
   explicit, reviewed removal decision.
9. Legacy URLs that are likely to be referenced lead readers to the new
   canonical page.
10. The strict build passes locally and in pull-request CI.
11. Desktop, narrow-screen, keyboard, search, table, code, image, and diagram
    behavior has been inspected.
12. Merging approved Markdown deploys the unchanged built artifact to GitHub
    Pages.
13. Reverting the documentation commit restores the previous published site
    without application or data migration.

## Implementation sequence

The recommended order is:

1. Approve this design.
2. Inventory current documentation and create the claim/migration audit.
3. Add the minimal pinned MkDocs toolchain and local strict build.
4. Create the navigation frame and home page.
5. Rewrite system architecture and the fastVEP section.
6. Rewrite annotation validation and scientific correctness pages.
7. Split and rewrite results, evidence, and phenotype documentation.
8. Rewrite user and maintainer workflows.
9. Validate content completeness, rendering, accessibility, and links.
10. Add pull-request build checks and default-branch Pages deployment.
11. Publish a preview, review it, and enable the canonical URL.
12. Use the manual task workflow for future updates.

The steps should be separate reviewable commits where practical. The existing
documentation remains available until its replacement has passed content and
link review.

## Rollback

The wiki is a static artifact derived from Git. If a publication is incorrect:

1. Revert the documentation commit or correct it in a new reviewed commit.
2. Let the Pages workflow rebuild the prior or corrected state.
3. Verify the canonical URL and affected deep links.

No database, migration, generated source, or application release rollback is
required.

## Deferred enhancements

Consider these only after the initial wiki is in regular use:

- multi-version documentation when users need to browse several releases;
- automated external-link checking if link rot becomes a recurring problem;
- generated CLI or schema reference when stable machine-readable inputs exist;
- contributor and last-update plugins if Git history is insufficient;
- offline documentation packaged with releases;
- custom AnnoCAT styling after the default Material theme has been usability
  tested; and
- an AI question interface only after defining retrieval scope, citations,
  privacy, evaluation, cost, and failure behavior.

## Platform references

- [Material for MkDocs: creating a site](https://squidfunk.github.io/mkdocs-material/creating-your-site/)
- [Material for MkDocs: navigation](https://squidfunk.github.io/mkdocs-material/setup/setting-up-navigation/)
- [Material for MkDocs: client-side search](https://squidfunk.github.io/mkdocs-material/setup/setting-up-site-search/)
- [Material for MkDocs: Mermaid diagrams](https://squidfunk.github.io/mkdocs-material/reference/diagrams/)
- [GitHub Pages custom workflows](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages)
- [OpenAI: keep documentation up to date with Codex](https://learn.chatgpt.com/use-cases/update-documentation)
- [OpenAI: Codex GitHub Action](https://learn.chatgpt.com/docs/github-action)

The Codex GitHub Action is referenced for capability context only. This design
does not use it for authoring because documentation updates are intentionally
manual.
