# Prediction evidence display

Status: Active policy; implemented rules are tested, and unresolved score identities remain neutral  
Last updated: 2026-08-17  
Applies to: AnnoCAT result tables, variant details, tooltips, filters, sorting, and prediction summaries

## Verification record

The numeric thresholds in this document were checked directly against the following primary tables:

- Pejaver et al. 2022, Table 2, for the original calibration of 13 missense predictors; the methods identify the calibrated PolyPhen-2 model as HumVar.[^pejaver-2022-table2]
- Bergquist et al. 2025, Table 1, for AlphaMissense, ESM1b, VARITY_R, and the additional 3-point intervals for previously calibrated predictors.[^bergquist-2025-table1]
- Walker et al. 2023 and the official ClinGen response to feedback for the generic SpliceAI cutoffs and intended scope.[^walker-2023][^clingen-splicing-feedback]

Verification here means that the numeric boundaries match the cited table. Runtime use still requires an exact score identity, compatible model or source release, applicable consequence, and correct allele and transcript.

The verified PolyPhen-2 calibration is HumVar, not HumDiv. The ESM1b
indeterminate interval ends at `-6.4`, not `-6.2`. The 2025 intervals below use
the decimal precision reported in the source table; AnnoCAT does not convert
adjacent displayed intervals into continuous comparisons.

The non-calibrated display rules were checked against primary or first-party documentation for CADD, SpliceAI, AlphaMissense, SIFT, PolyPhen-2, REVEL, BayesDel, PrimateAI, phyloP, GERP++, and dbNSFP field identities.[^cadd-interpretation][^spliceai-developer][^alphamissense-developer][^sift-developer][^polyphen-developer][^revel-developer][^bayesdel-developer][^primateai-developer][^phylop-semantics][^gerp-semantics][^dbnsfp-source] These rules are display interpretations, not additional ACMG/AMP criteria. Where a source documents rank or direction but no clinically calibrated cutoff, the document labels the resulting color rule as an AnnoCAT display mapping.

Presentation follows WCAG requirements and Fluent 2 semantic-color guidance.
Color is supplementary rather than the sole carrier of meaning. Ordinary-size
text must meet at least 4.5:1 contrast, and meaningful non-text indicators must
meet at least 3:1 contrast. Status colors are reserved for meaningful states,
not decoration.[^wcag-color][^wcag-contrast][^fluent-color]

## 1. Decision

AnnoCAT uses five interpretation tiers for computational evidence:

1. Use published ClinGen Sequence Variant Interpretation (SVI) calibrated score intervals when the exact predictor, score field, model or release, and variant type match the calibration.
2. Otherwise, use a verified developer or source-native category when it is explicitly defined for the exact field.
3. Otherwise, use documented directional or rank context only when the exact score identity is known and the source supports that interpretation.
4. Show undirected measurements and unresolved numeric scores as neutral text.
5. Show missing or inapplicable values with the missing-data treatment.

Interpretation tier and biological direction are separate visual dimensions:

| Interpretation tier | Visual treatment | Use |
| --- | --- | --- |
| Published calibrated interpretation | Soft status pill | Exact approved calibration, release, field, allele, transcript, and consequence |
| Verified source-native interpretation | Plain colored text | Exact developer threshold/category or documented source-derived category |
| Directional or rank context | Plain colored or neutral text | Documented direction or rank without calibrated clinical evidence strength |
| Neutral measurement | Default dark text | Available data with no defensible adverse or reassuring interpretation |
| Missing or not applicable | Muted gray text | No value, field not returned, source unavailable, or interpretation not applicable |

Hue has one meaning across all tiers:

| Direction | Tone | Meaning |
| --- | --- | --- |
| Adverse | Red | Pathogenic, damaging, deleterious, failed quality, or an explicitly documented high adverse rank |
| Caution | Amber | An explicitly uncertain source category, intermediate adverse rank, or review-worthy technical state |
| Reassuring | Green | Benign, tolerated, neutral by a verified source category, or passed configured quality |
| No direction | Default dark text | Available context that is neither adverse nor reassuring |
| Missing | Gray | Unavailable, unreported, invalid, or not applicable |

Blue is reserved for links, actions, selection, explicit phenotype or candidate-gene matches, and rare informational badges. It is not the default color for available evidence.

Calibration status is communicated by the pill shape, tooltip prefix, and accessible label. Strength is communicated by text and tooltip, not by changing hue or saturation. All pathogenic-direction calibrated bands are red pills, all benign-direction calibrated bands are green pills, and calibrated indeterminate bands are neutral pills. A supporting-pathogenic interval must not become amber merely because its evidence strength is supporting.

An `indeterminate` interval in a ClinGen calibration uses a neutral pill. The
score does not support either PP3 or BP4 at the calibrated evidence levels; it
is not an amber warning or an uncertain clinical classification.[^pejaver-2022-table2]
Amber remains available for an exact source category such as `uncertain` or
`possibly damaging`, and for a documented intermediate adverse rank.

The UI encodes two properties:

- **Was the score interpreted through an approved calibration?** Pill means yes; plain text means no.
- **What direction did that interpretation support?** Red means adverse, green means reassuring, amber means explicit caution or uncertainty, and neutral means no direction.

The pill is a noninteractive status badge, not a button or editable tag. Fluent recommends badges for short, system-generated status and advises using semantic color intentionally and consistently.[^fluent-badge][^fluent-color] The value must remain readable without relying on the tint alone.

Color is a display aid. It is not an AnnoCAT pathogenicity classification.

A value receives semantic color only when its exact identity, applicability,
and interpretation source justify a direction or caution state. Available but
undirected data remain neutral so red, amber, and green retain meaning.

## 2. Non-goals

This policy does not:

- Automatically classify variants under ACMG/AMP.
- Implement FastVEP's automated 2015 ACMG rules.
- Convert a count of agreeing predictors into PP3 or BP4 evidence.
- Treat computational predictions as independent when they use correlated training data or features.
- Apply global calibration where a gene-specific Variant Curation Expert Panel specification supersedes it.
- Apply a predictor calibration to a different model, converted score, rank score, or release without verification.
- Treat population frequency or conservation alone as a pathogenic or benign classification.

AnnoCAT remains a research and review application. The UI must not imply that its colors constitute clinical evidence adjudication.

## 3. Governing principles

### 3.1 One interpretation registry

The result table, variant details, filters, sorting labels, prediction summary, and tooltips must all use the same interpretation registry. A score must never be red in one component and amber or gray in another because separate thresholds were implemented.

Each registered field needs:

- Stable predictor ID.
- Human-readable label.
- Source ID.
- Exact source field name.
- Score identity.
- Model and release, where available.
- Applicable variant classes.
- Excluded variant classes.
- Interpretation source: ClinGen calibration, developer category, or contextual only.
- Calibration reference and version.
- Threshold intervals with explicit inclusive and exclusive boundaries.
- Interpretation tier.
- Direction independent of evidence strength.
- Visual treatment: calibrated pill, plain directional text, neutral text, or missing text.
- Summary inclusion policy.

### 3.2 Applicability before thresholds

The interpretation flow is:

1. Confirm the value is present and valid.
2. Confirm the exact predictor and score identity.
3. Confirm model and release compatibility.
4. Confirm the selected transcript consequence is within the calibration scope.
5. Confirm any explicit exclusions do not apply.
6. Apply the calibrated interval.
7. If calibration does not apply, use a verified developer or source-native category.
8. If neither applies, use documented directional or rank context only when the exact score identity supports it.
9. Otherwise show the value as neutral text.

The code must not apply a threshold first and check consequence later.

### 3.3 Selected transcript context

Missense calibrations apply only when the selected transcript has a missense consequence. AnnoCAT does not define a separate transcript order for evidence display. The default context uses the gene-first, two-stage `allele-gene-severity-v1` representative selected by [How AnnoCAT selects transcripts and evidence](transcript-and-evidence-selection.md). If the user selects another transcript in Variant Details, interpretation uses that transcript's consequence and matching transcript-scoped evidence.

A score available at the variant level does not make a non-missense selected transcript eligible for missense calibration.

### 3.4 Do not stack correlated predictors

ClinGen SVI recommends calibrated use of an individual predictor rather than consensus voting across many correlated tools.[^pejaver-2022-table4] AnnoCAT will therefore use:

- REVEL as the predeclared primary missense protein-effect predictor.
- SpliceAI maximum delta score as the predeclared primary splice-effect predictor.
- Other predictors as alternative display evidence, not additional independent PP3/BP4 votes.

The UI may show all available predictions. It must not add them together into an ACMG classification.

### 3.5 Variant summary and sequencing-quality colors

Variant interpretation and sequencing quality are separate domains. A red predictor means that the predictor points in a damaging direction. A red quality metric means that the call does not meet the active quality profile. Neither meaning is an ACMG/AMP classification.

Color must be applied to the value or status, not to the field label. Every colored value also needs text that communicates its meaning without color.

| Summary field | Display policy |
| --- | --- |
| Gene | Default text. Use blue only for a link or an explicitly labeled phenotype or candidate-gene match. A gene symbol alone does not receive a pathogenicity color. |
| Consequence | Keep the consequence text neutral. Show the selected transcript's verified VEP impact separately: HIGH red, MODERATE amber, LOW neutral, and MODIFIER muted. This is predicted effect severity, not clinical significance. `upstream_gene_variant` is MODIFIER.[^ensembl-consequences] |
| Zygosity | Default text for an available called state and gray for missing or not called. Zygosity is inheritance context and is not intrinsically benign or pathogenic. |
| ClinVar | Red for pathogenic or likely pathogenic, green for benign or likely benign, amber for uncertain or conflicting, neutral for other relationships such as risk factor or association, and gray for no assertion. |
| Population AF and group-max AF | Default text for available frequency context and gray for missing data. Apply green benign-frequency evidence only through an explicitly selected disease- and inheritance-specific rule. Rarity alone must not be red or treated as pathogenic evidence.[^clingen-pm2] |
| Conservation and computational predictors | Use the registry and applicability rules in this document. A calibrated interpretation uses a pill; a verified source-native interpretation or directional context uses plain text; otherwise use neutral text. |
| Prediction summary | Use the same normalized interpretations as the detailed predictor rows. Missing data are gray and contextual-only values do not enter red, amber, or green counts. |
| Genotype and phase | Use default text for a called genotype. Show phase as secondary context; unphased is not a failure. Missing genotype is gray. |
| Depth, GQ, allelic support, FILTER, and QUAL | Use the sequencing-quality policy below. Do not apply predictor or clinical-significance colors to these metrics. |

#### 3.5.1 Quality profiles

Quality thresholds depend on specimen type, assay, caller, variant class, and validated workflow. ACMG technical standards therefore recommend that each laboratory establish its own minimum coverage and allele-fraction parameters; a single universal depth or allelic-balance threshold is not appropriate.[^acmg-ngs-2021]

AnnoCAT must evaluate quality metrics through an explicit quality profile:

- The result records the profile ID, version, and thresholds used.
- A caller- or assay-specific profile takes precedence over an AnnoCAT display default.
- A metric without an applicable threshold is neutral context, not an inferred pass.
- Missing quality data are gray and must not be converted to zero.
- SNVs, indels, mitochondrial variants, mosaic calls, and somatic calls may require different profiles.

| Quality result | Tone | Meaning |
| --- | --- | --- |
| Meets the active profile | Green | The individual metric meets the configured display threshold |
| Near a configured boundary | Amber | The metric warrants review but has not crossed the configured failure threshold |
| Fails the active profile | Red | The individual metric does not meet the configured display threshold |
| No applicable profile threshold | Default dark text | Available quality context with no inferred pass or failure |
| Missing | Gray | The source did not provide the metric |

#### 3.5.2 Metric-specific behavior

VCF fields describe different levels of the call and must not be collapsed into a single pass state. `FILTER` is the record's filter status, `QUAL` is site-level call confidence, `GQ` is sample genotype confidence, `DP` is sample depth, and `AD` contains per-allele depths in REF, ALT1, ALT2 order.[^gatk-vcf-fields] GATK also cautions that AD includes caller-filtered evidence and should not by itself be treated as a genotype decision.[^gatk-ad]

| Metric | Required behavior |
| --- | --- |
| VCF FILTER | `PASS` is green. A named failed filter is red unless the active caller profile explicitly classifies it as a warning. Missing or unevaluated FILTER is gray. |
| QUAL | Apply color only through a caller-specific profile. Otherwise show the value in default text. QUAL must not inherit the FILTER color. |
| Depth (DP) | Compare with the active assay and variant-class profile. Adequate depth is green, cautionary depth is amber, and depth below the configured minimum is red. |
| Genotype quality (GQ) | Compare with the active caller profile independently of DP and QUAL. A high site QUAL does not rescue a poor sample GQ. |
| Allelic support (AD and allele balance) | Interpret against the called genotype and active profile. A heterozygous call is expected near balanced representation, while homozygous-alternate and mosaic calls require different expectations. Also evaluate the absolute alternate-read count where available. |
| Genotype and phase | Preserve the called value as context. Poor QC adds a warning but does not silently rewrite `0/1`, `1/1`, phased, or unphased. |

#### 3.5.3 Failed QC and combined cells

`PASS` does not mean that every sample-level quality metric is adequate. A record can pass site filters while the selected sample has poor depth, low GQ, or imbalanced allelic support.

The UI must therefore:

1. Evaluate FILTER, QUAL, DP, GQ, alternate-read count, and allele balance independently.
2. Color each part of a combined cell separately.
3. Preserve all failed and cautionary reasons in the tooltip and accessible label.
4. Derive an optional aggregate call-quality status from the worst applicable metric: failed if any metric fails, caution if none fail and at least one warns, pass only if all required metrics pass, and unavailable if required metrics are missing.
5. Never use the aggregate QC status in the pathogenicity prediction summary.

Examples:

- `PASS` plus poor depth renders `PASS` in green and `Low depth` in red. The aggregate QC status is `Needs review`, not `PASS`.
- A failed VCF filter plus poor depth shows both failure reasons. One must not hide the other.
- `Depth / GQ` renders two independently colored values. Poor depth remains red even when GQ passes.
- For a heterozygous `0/1` call with `18 / 2` REF/ALT reads and 10% alternate balance, a validated germline profile may flag both the low alternate-read count and imbalance. Without an applicable profile, those values remain neutral rather than receiving an invented universal threshold.

### 3.6 Fluent semantic presentation

AnnoCAT must implement evidence colors through semantic aliases, not predictor-specific hex values or component-local overrides. Fluent defines neutral, brand, and shared semantic palettes and recommends reserving semantic status colors for meaningful feedback rather than decoration.[^fluent-color][^fluent-tokens]

Use one foreground role per meaning and one soft pill recipe per calibrated meaning:

| AnnoCAT role | Fluent-compatible semantic role | Use |
| --- | --- | --- |
| Adverse plain text | Danger foreground | Verified adverse source category or high adverse rank |
| Caution plain text | Warning foreground | Explicit uncertainty, intermediate rank, or technical caution |
| Reassuring plain text | Success foreground | Verified benign, tolerated, or passed state |
| Neutral value | Primary neutral foreground | Available value without direction |
| Missing value | Secondary or disabled neutral foreground | Missing, invalid, or not applicable |
| Calibrated adverse pill | Danger foreground + soft danger background + danger border | Calibrated pathogenic-direction interval |
| Calibrated reassuring pill | Success foreground + soft success background + success border | Calibrated benign-direction interval |
| Calibrated indeterminate pill | Neutral foreground + subtle neutral background + neutral border | Score evaluated by the calibration but supporting neither direction |
| Interaction | Brand foreground or brand fill | Links, actions, focus, selection, and explicit candidate matches |

The AnnoCAT theme should expose these as product aliases, for example `--annocat-evidence-adverse-foreground`, and map them to the active Fluent-compatible light, dark, or high-contrast theme. Components consume only the product aliases. They must not choose a darker green for one predictor, a brighter green for another, or an informational blue because a score is available.

Plain directional values inherit the surrounding value typography; color must not make a selected row or interpreted value bolder. Calibrated pills use the same value type ramp with a single consistent badge weight.

For Fluent-compatible implementations, the intended starting roles are `colorStatusDangerForeground1`, `colorStatusWarningForeground1`, `colorStatusSuccessForeground1`, the corresponding `Background1` and `Border1` status tokens for calibrated pills, and neutral foreground, background, and stroke tokens for indeterminate and missing states.[^fluent-tokens] Exact token mapping still requires contrast testing in every AnnoCAT theme. Do not substitute pure yellow text on white merely to make amber look more yellow; use a warning foreground that preserves the required contrast and is visually distinct from the danger token.

To avoid color becoming the only cue:

- Variant details show a visible interpretation label with the score.
- Dense table cells expose the same short interpretation in a mouse- and keyboard-accessible tooltip and accessible name.
- Calibrated values retain the pill shape in both the table and details.
- Filters expose normalized text categories such as `Calibrated pathogenic`, `Developer uncertain`, and `Neutral measurement`.
- Prediction-summary segments have visible labels or an adjacent legend, not color alone.
- The same value, allele, transcript, tier, direction, tooltip, and component treatment are reused in every view.

Fluent also advises against mixing badge sizes in one context.[^fluent-badge] Calibrated pills therefore use one compact size in table cells and one consistent regular size in detail rows; different evidence strengths do not change pill size, fill saturation, typography, or elevation. Pills have no shadow, pressed state, or button-like hover treatment.

## 4. Evidence tiers

### Tier A: Published calibrated interpretation

Requirements:

- Exact original score field.
- Compatible predictor model or release.
- Applicable variant type.
- Published threshold table.
- Calibration approved in AnnoCAT's registry.

Display:

- Soft status pill around the value.
- Green pill for calibrated benign direction.
- Neutral pill for the calibrated indeterminate interval.
- Red pill for calibrated pathogenic direction, including supporting-strength intervals.
- Visible or accessible text identifies the interpretation as calibrated.
- Tooltip names the score, direction, evidence strength, scope, and calibration source.

Example:

`REVEL 0.72 - calibrated supporting pathogenic range`

The pill, rather than a special hue, distinguishes calibrated evidence from other predictions. Evidence strength does not change the hue. The neutral pill means `calibration applied; no calibrated direction`, not missing data.

### Tier B: Verified source-native interpretation

Requirements:

- Exact category field or a documented source-native threshold.
- Field-specific code mapping.
- Compatible predictor release.
- Recorded category origin: developer-provided or aggregator-derived.

Display:

- Plain text; no calibrated pill.
- Green for benign, tolerated, or neutral.
- Amber for uncertain, ambiguous, intermediate, or possibly damaging.
- Red for pathogenic, damaging, deleterious, or disease-causing.
- Tooltip identifies whether this is a developer category or a source-derived category and explicitly states that it is not calibrated evidence.

Example:

`SIFT 0.03 - deleterious by the developer threshold`

Source-derived categories such as a dbNSFP `_pred` field can use this tier only when the exact field mapping is registered. They must not be described as categories issued by the original predictor when dbNSFP derived the category itself.

### Tier C: Documented directional or rank context

Use when a numeric value is valid, its exact identity is known, and its source documents useful direction or rank, but no approved calibrated interpretation applies.

Display:

- Plain text; no calibrated pill.
- Red or amber only for the documented adverse or cautionary direction defined in the predictor-specific matrix.
- Green only when the source explicitly defines a benign, tolerated, or neutral category. A low value is not automatically reassuring.
- Default dark text when the context is undirected.
- Concise tooltip starts with `Contextual rank` or `Directional context`.
- No calibrated, PP3, BP4, supporting, moderate, strong, benign-evidence, or pathogenic-evidence label.

Examples:

- Non-missense CADD PHRED with exact score identity.
- Gene.iobio-style population-frequency and phyloP triage bands outside an approved calibration scope.
- Exact SpliceAI maximum delta with verified model settings when generic calibration is inapplicable.

### Tier D: Neutral measurement or unresolved score

Use for:

- Available context without a defensible adverse or reassuring interpretation.
- A valid predictor value whose upstream model or release is unresolved.
- Population frequency when no registered triage band applies.
- Genotype, phase, depth, GQ, QUAL, or allelic support without an applicable quality profile.
- Conservation values for which only the raw evolutionary measurement is defensible.

Display:

- Default dark text.
- No pill.
- Tooltip explains what the value measures and why no directional interpretation was applied.

Examples:

- A FAVOR REVEL score whose upstream REVEL release is not recorded.
- An unverified ESM1b or legacy MutPred field.
- Positive phyloP or any uncalibrated GERP++ RS value.

### Tier E: Missing or not applicable

Display:

- Gray dash in dense tables.
- Clear text such as `Not reported` in details when necessary for comprehension.
- Tooltip distinguishes missing, not returned, source unavailable, and not applicable when that distinction is known.

Never convert a missing value to numeric zero.

## 5. Published calibration matrix

### 5.1 Verification status

The 2022 ClinGen SVI bands below match Pejaver et al. 2022, Table 2.[^pejaver-2022-table2] That table publishes the benign and pathogenic bands; where the document shows an indeterminate interval for a 2022 tool, it is the open complement between the published supporting-benign and supporting-pathogenic boundaries. The 2025 AlphaMissense, VARITY_R, and ESM1b bands match Bergquist et al. 2025, Table 1.[^bergquist-2025-table1] The 2023 SpliceAI cutoffs match the ClinGen SVI splicing guidance and its official clarification for generic use.[^walker-2023][^clingen-splicing-feedback]

Numeric verification does not establish source compatibility. A verified threshold remains inactive for a result field when AnnoCAT cannot establish that the field is the exact score and model used by the calibration.

The 2025 paper also reports 3-point intervals for an anticipated point-based framework. These intervals are valid published calibration ranges, but `3-point` is not an evidence-strength category in the 2015 ACMG/AMP framework. As of this document's 2026-07-26 review, the current ClinGen guidance portal does not provide a final replacement standard that would authorize AnnoCAT to convert those intervals into a new classification rule.[^clingen-current-guidance] AnnoCAT must preserve the source label and must not call them `moderate-to-strong ACMG evidence`.

Bergquist Table 1 reports AlphaMissense and VARITY_R to three decimal places and ESM1b to one decimal place. AnnoCAT must preserve those exact published intervals. It may round a higher-precision runtime value to the table precision only after verifying that the matched source uses the same score definition and rounding contract. Without that contract, values in a precision gap remain outside the calibrated bands rather than being interpolated into either neighboring band.

### 5.2 REVEL

Scope: original REVEL score for missense variants  
Role: primary missense predictor  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.003` | Benign | Very strong |
| `> 0.003 and <= 0.016` | Benign | Strong |
| `> 0.016 and <= 0.183` | Benign | Moderate |
| `> 0.183 and <= 0.290` | Benign | Supporting |
| `> 0.290 and < 0.644` | Indeterminate | None |
| `>= 0.644 and < 0.773` | Pathogenic | Supporting |
| `>= 0.773 and < 0.932` | Pathogenic | Moderate |
| `>= 0.932` | Pathogenic | Strong |

Excluded identities:

- REVEL rank score.
- Converted or normalized REVEL values.
- Scores whose upstream REVEL release cannot be established when release compatibility matters.

### 5.3 BayesDel without allele frequency

Scope: BayesDel no-AF score for missense variants  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= -0.360` | Benign | Moderate |
| `> -0.360 and <= -0.180` | Benign | Supporting |
| `> -0.180 and < 0.130` | Indeterminate | None |
| `>= 0.130 and < 0.270` | Pathogenic | Supporting |
| `>= 0.270 and < 0.500` | Pathogenic | Moderate |
| `>= 0.500` | Pathogenic | Strong |

Do not substitute the allele-frequency model or a rank score.

### 5.4 VEST4

Scope: original VEST4 score for missense variants  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.302` | Benign | Moderate |
| `> 0.302 and <= 0.449` | Benign | Supporting |
| `> 0.449 and < 0.764` | Indeterminate | None |
| `>= 0.764 and < 0.861` | Pathogenic | Supporting |
| `>= 0.861 and < 0.965` | Pathogenic | Moderate |
| `>= 0.965` | Pathogenic | Strong |

### 5.5 PrimateAI

Scope: original PrimateAI score for missense variants  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.362` | Benign | Moderate |
| `> 0.362 and <= 0.483` | Benign | Supporting |
| `> 0.483 and < 0.790` | Indeterminate | None |
| `>= 0.790 and < 0.867` | Pathogenic | Supporting |
| `>= 0.867` | Pathogenic | Moderate |

### 5.6 PolyPhen-2 HVAR

Scope: original PolyPhen-2 HumVar score for missense variants  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.009` | Benign | Moderate |
| `> 0.009 and <= 0.113` | Benign | Supporting |
| `> 0.113 and < 0.978` | Indeterminate | None |
| `>= 0.978 and < 0.999` | Pathogenic | Supporting |
| `>= 0.999` | Pathogenic | Moderate |

PolyPhen-2 HumDiv is a different model and must not use this HumVar calibration.

### 5.7 SIFT

Scope: original SIFT score for missense variants  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

Lower SIFT scores indicate a more damaging prediction.

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.000` | Pathogenic | Moderate |
| `> 0.000 and <= 0.001` | Pathogenic | Supporting |
| `> 0.001 and < 0.080` | Indeterminate | None |
| `>= 0.080 and < 0.327` | Benign | Supporting |
| `>= 0.327` | Benign | Moderate |

SIFT4G, converted scores, categorical fields, and rank scores are separate identities.

### 5.8 CADD PHRED

Scope: CADD PHRED score for missense variants only  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.15` | Benign | Strong |
| `> 0.15 and <= 17.3` | Benign | Moderate |
| `> 17.3 and <= 22.7` | Benign | Supporting |
| `> 22.7 and < 25.3` | Indeterminate | None |
| `>= 25.3 and < 28.1` | Pathogenic | Supporting |
| `>= 28.1` | Pathogenic | Moderate |

Outside a missense consequence, the missense calibration must not be applied. When the exact CADD PHRED identity is established, use the following AnnoCAT rank-display mapping:

| CADD PHRED | Display | Tooltip context |
| --- | --- | --- |
| `< 10` | Neutral text | Below the top 10 percent rank milestone |
| `>= 10 and < 20` | Amber plain text | Between the top 10 percent and top 1 percent rank milestones |
| `>= 20` | Red plain text | Within approximately the top 1 percent of possible substitutions |
| `>= 30` | Red plain text | Within approximately the top 0.1 percent of possible substitutions |

The `>= 30` rule does not use a darker red or a pill; the stronger rank statement belongs in the tooltip. CADD values below 10 are not green because low CADD is not an independently validated benign category.

CADD's PHRED score is rank-scaled:

- `10` means approximately the top 10 percent of possible substitutions.
- `20` means approximately the top 1 percent.
- `30` means approximately the top 0.1 percent.

These rank descriptions belong in the tooltip, not in the table cell. The amber and red mapping is an AnnoCAT visual prioritization policy based on these official rank milestones, not a CADD clinical threshold. CADD states that there is no natural universal cutoff and recommends integrating its scores with other evidence rather than hard filtering.[^cadd-interpretation] A high CADD score on a stop-gain variant can therefore be red plain text, but it must never be called calibrated PP3 evidence or receive a calibrated pill.

The applicable calibrated interpretation always takes precedence over this contextual rank mapping. For example, an exact missense CADD PHRED score of `21` falls in the calibrated supporting-benign interval and is shown as a green calibrated pill; the same numeric score outside missense scope is red plain text only because it is in approximately the top 1 percent rank. The UI must show only the applicable result, explain the decision path in the tooltip, and never blend the two interpretations.

### 5.9 MPC

Scope: original MPC score for missense variants  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `< 1.360` | Indeterminate | None |
| `>= 1.360 and < 1.828` | Pathogenic | Supporting |
| `>= 1.828` | Pathogenic | Moderate |

No calibrated benign interval is defined here.

### 5.10 phyloP 100-way vertebrate

Scope: phyloP 100-way vertebrate score for missense variants  
Role: contextual alternative, not stacked with REVEL  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.021` | Benign | Moderate |
| `> 0.021 and <= 1.879` | Benign | Supporting |
| `> 1.879 and < 7.367` | Indeterminate | None |
| `>= 7.367 and < 9.741` | Pathogenic | Supporting |
| `>= 9.741` | Pathogenic | Moderate |

Outside missense scope, display phyloP as conservation context:

- `<= 0`: neutral plain text, labeled not conserved.
- `> 0` and `<= 1`: green plain text, labeled marginally conserved.
- `> 1` and `<= 1.5`: amber plain text, labeled moderately conserved.
- `> 1.5`: red plain text, labeled highly conserved.
- Do not imply pathogenicity from conservation alone.

Positive phyloP scores indicate conservation and negative scores indicate faster-than-expected evolution.[^phylop-semantics] The non-calibrated bands copy Gene.iobio's open-source variant-inspection display rules.[^geneiobio-triage] Their colors indicate triage attention, not benign or pathogenic evidence.

As with CADD, calibrated scope takes precedence. A phyloP score of `-1.66` on a verified missense context falls in the calibrated benign interval and is a green calibrated pill. Outside that scope it is neutral plain text under the Gene.iobio display mapping. The result table and variant details must use the same selected allele, transcript, consequence, source identity, and interpretation object, so they cannot show two treatments for the same selection.

### 5.11 GERP++ rejected-substitution score

Scope: GERP++ RS for missense variants  
Role: contextual alternative, not stacked with REVEL  
Reference: Pejaver et al. 2022, Table 2.[^pejaver-2022-table2]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= -4.54` | Benign | Moderate |
| `> -4.54 and <= 2.70` | Benign | Supporting |
| `> 2.70` | Indeterminate | None |

No calibrated pathogenic interval is defined here.

Outside the approved missense calibration, all GERP++ RS values remain neutral text. Positive values indicate a substitution deficit and greater evolutionary constraint. Negative values indicate a substitution surplus, but the UCSC GERP documentation cautions that negative values should not be interpreted as accelerated evolution because alignment uncertainty and rate variance are strong confounders.[^gerp-semantics] AnnoCAT therefore does not apply the phyloP negative-score amber rule to GERP++.

### 5.12 SpliceAI maximum delta score

Scope: maximum of `DS_AG`, `DS_AL`, `DS_DG`, and `DS_DL` for the selected allele  
Role: primary splice-effect predictor  
Reference: Walker et al. 2023 and the ClinGen SVI response to feedback.[^walker-2023][^clingen-splicing-feedback]

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.10` | Non-spliceogenic | Moderate |
| `> 0.10 and < 0.20` | Indeterminate | None |
| `>= 0.20` | Spliceogenic | Moderate |

The generic calibration excludes canonical splice donor and acceptor variants. Canonical splice-site variants require separate loss-of-function assessment and must not receive this generic color interpretation.

When the generic calibration is inapplicable but the exact maximum-delta score, masking mode, annotation set, and model identity are verified, the source-native display can use the SpliceAI developer landmarks:

| Maximum delta | Display | Meaning |
| --- | --- | --- |
| `< 0.20` | Neutral text | Below the developer's high-recall landmark; not a benign category |
| `>= 0.20 and < 0.50` | Amber plain text | At or above the high-recall landmark |
| `>= 0.50` | Red plain text | At or above the developer's recommended landmark |
| `>= 0.80` | Red plain text | At or above the developer's high-precision landmark |

The developer characterizes `0.20`, `0.50`, and `0.80` as high-recall, recommended, and high-precision cutoffs, respectively.[^spliceai-developer] These are splice-effect prediction landmarks, not generic pathogenicity classifications. Scores below `0.20` remain neutral rather than green unless the approved ClinGen calibration applies.

### 5.13 AlphaMissense

Scope: original AlphaMissense pathogenicity score for missense variants  
Reference: Bergquist et al. 2025, Table 1.[^bergquist-2025-table1]  
Verification state: numeric intervals verified; source and model identity must still be established at runtime

Verified intervals:

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.070` | Benign | 3-point interval |
| `[0.071, 0.099]` | Benign | Moderate |
| `[0.100, 0.169]` | Benign | Supporting |
| `[0.170, 0.791]` | Indeterminate | None |
| `[0.792, 0.905]` | Pathogenic | Supporting |
| `[0.906, 0.971]` | Pathogenic | Moderate |
| `[0.972, 0.989]` | Pathogenic | 3-point interval |
| `>= 0.990` | Pathogenic | Strong |

These are the source table's discrete three-decimal intervals.[^bergquist-2025-table1] A value such as `0.0705` must not be assigned to a calibrated band unless the matched source's precision and rounding contract has been verified.

If the exact calibrated model identity cannot be established, use the developer categories instead:

- `< 0.34`: likely benign.
- `0.34 through 0.564`: uncertain.
- `> 0.564`: likely pathogenic.

These developer categories come from AlphaMissense and are display-only; they must not be labeled PP3 or BP4.[^alphamissense-developer]

### 5.14 VARITY_R

Scope: original VARITY_R rare-variant model score for missense variants  
Reference: Bergquist et al. 2025, Table 1.[^bergquist-2025-table1]  
Verification state: numeric intervals verified; VARITY_R identity and source compatibility must still be established at runtime

Verified intervals:

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.036` | Benign | Strong |
| `[0.037, 0.063]` | Benign | 3-point interval |
| `[0.064, 0.116]` | Benign | Moderate |
| `[0.117, 0.251]` | Benign | Supporting |
| `[0.252, 0.674]` | Indeterminate | None |
| `[0.675, 0.841]` | Pathogenic | Supporting |
| `[0.842, 0.914]` | Pathogenic | Moderate |
| `[0.915, 0.964]` | Pathogenic | 3-point interval |
| `>= 0.965` | Pathogenic | Strong |

These are the source table's discrete three-decimal intervals.[^bergquist-2025-table1] VARITY_ER is a different model and must not use these intervals. Higher-precision values remain uncalibrated unless the matched source precision and rounding contract has been verified.

### 5.15 ESM1b - thresholds verified, field identity unresolved

Scope: the exact ESM1b score calibrated by Bergquist et al. for missense variants  
Reference: Bergquist et al. 2025, Table 1.[^bergquist-2025-table1]  
Verification state: numeric intervals verified; the current dbNSFP field identity and precision have not been matched to the calibration input

Lower ESM1b scores indicate a more pathogenic prediction in this table.

| Score | Direction | Strength |
| --- | --- | --- |
| `>= 8.8` | Benign | 3-point interval |
| `[-3.1, 8.7]` | Benign | Moderate |
| `[-6.3, -3.2]` | Benign | Supporting |
| `[-10.6, -6.4]` | Indeterminate | None |
| `[-12.1, -10.7]` | Pathogenic | Supporting |
| `[-13.9, -12.2]` | Pathogenic | Moderate |
| `[-23.9, -14.0]` | Pathogenic | 3-point interval |
| `<= -24.0` | Pathogenic | Strong |

These are the source table's discrete one-decimal intervals.[^bergquist-2025-table1] The calibrated display must remain disabled until AnnoCAT verifies that its ESM1b field has the same score definition, direction, transcript mapping, precision, and rounding contract.

### 5.16 Other published 2022 calibrations

Pejaver et al. 2022, Table 2 also reports calibrated intervals for MutPred2, Evolutionary Action, and FATHMM.[^pejaver-2022-table2] MutPred2 is registered for the exact `mutpred2.score` exposed by the FAVOR dbNSFP 5.3.1a coding contract. The other tables remain reference values, not approval to apply them to similarly named AnnoCAT fields.

#### MutPred2

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.010` | Benign | Strong |
| `> 0.010 and <= 0.197` | Benign | Moderate |
| `> 0.197 and <= 0.391` | Benign | Supporting |
| `> 0.391 and < 0.737` | Indeterminate | None |
| `>= 0.737 and < 0.829` | Pathogenic | Supporting |
| `>= 0.829 and < 0.932` | Pathogenic | Moderate |
| `>= 0.932` | Pathogenic | Strong |

The FAVOR coding `mutpred2.score` uses this table. The installed dbNSFP 4.9a `MutPred_score` remains legacy MutPred v1.2 and must not use it.

#### Evolutionary Action

| Score | Direction | Strength |
| --- | --- | --- |
| `<= 0.069` | Benign | Moderate |
| `> 0.069 and <= 0.262` | Benign | Supporting |
| `> 0.262 and < 0.685` | Indeterminate | None |
| `>= 0.685 and < 0.821` | Pathogenic | Supporting |
| `>= 0.821` | Pathogenic | Moderate |

#### FATHMM

Lower FATHMM scores indicate a more pathogenic prediction in this calibration.

| Score | Direction | Strength |
| --- | --- | --- |
| `>= 4.69` | Benign | Moderate |
| `>= 3.32 and < 4.69` | Benign | Supporting |
| `> -4.14 and < 3.32` | Indeterminate | None |
| `> -5.04 and <= -4.14` | Pathogenic | Supporting |
| `<= -5.04` | Pathogenic | Moderate |

FATHMM, fathmm-MKL, and fathmm-XF are different predictors and must not share calibration thresholds.

## 6. Non-calibrated display matrix

A non-calibrated interpretation is allowed only when the exact field identity, variant-type applicability, source category or threshold, and code mapping are registered. These displays never use a calibrated pill and never contribute calibrated evidence strength.

| Predictor or field | Non-calibrated interpretation | Display |
| --- | --- | --- |
| AlphaMissense exact score | `< 0.34` likely benign; `0.34-0.564` uncertain; `> 0.564` likely pathogenic; missense only | Green, amber, red plain text |
| AlphaMissense exact category | Likely benign, uncertain, or likely pathogenic | Green, amber, red plain text |
| BayesDel no-AF exact score | `< -0.0570105` tolerated; `>= -0.0570105` deleterious; research prioritization cutoff, missense only | Green or red plain text |
| PrimateAI exact score | `< 0.60` likely benign; `0.60-0.80` intermediate; `> 0.80` likely pathogenic; missense only | Green, amber, red plain text |
| SIFT exact score | `< 0.05` deleterious; `>= 0.05` tolerated; amino-acid substitution only | Red or green plain text |
| SIFT or SIFT4G exact category | Deleterious or tolerated | Red or green plain text |
| PolyPhen-2 HDIV `_pred` | Use the exact HumDiv category; never apply the Pejaver HumVar calibration | Green, amber, red plain text |
| PolyPhen-2 HVAR `_pred` | Use the exact HumVar category; the numeric HumVar calibration is separate from this source-native category | Green, amber, red plain text |
| PolyPhen-2 HVAR exact score | `<= 0.446` benign; `> 0.446-0.908` possibly damaging; `> 0.908` probably damaging; missense only | Green, amber, red plain text |
| REVEL exact score without approved calibration | `< 0.50` likely benign source-derived range; `>= 0.50` sensitivity-oriented likely disease-causing range; `>= 0.75` higher-specificity likely disease-causing range; missense only | Green or red plain text |
| SpliceAI exact maximum delta | `< 0.20` below high-recall landmark; `0.20-<0.50` high-recall; `>= 0.50` recommended; `>= 0.80` high-precision | Neutral, amber, red plain text |
| CADD PHRED outside missense | `< 10` below top-10-percent milestone; `10-<20` elevated rank; `>= 20` top 1 percent; `>= 30` top 0.1 percent | Neutral, amber, red plain text |
| gnomAD and other population AF | `<= 0.01` rare; `> 0.01-0.05` uncommon; `> 0.05` common | Red, amber, or green plain text |
| phyloP outside approved calibration | `<= 0` not conserved; `> 0-1` marginally conserved; `> 1-1.5` moderately conserved; `> 1.5` highly conserved | Neutral, green, amber, or red plain text |
| FAVOR conservation aPC | `< 10` below top 10%; `10-<20` top 10%; `20-<30` top 1%; `>= 30` top 0.1% | Green, amber, or red plain text |
| GERP++ RS outside approved calibration | Positive indicates constraint; negative values are not treated as acceleration because of documented confounding | Neutral text |
| Exact dbNSFP `_pred` field | Use only its registered field-specific category mapping | Plain direction-colored text |
| Unverified numeric predictor or unresolved FAVOR upstream release | No threshold-based interpretation | Neutral text |

The REVEL authors presented `0.50` and `0.75` as alternative pathogenicity thresholds trading sensitivity for specificity.[^revel-developer] Ensembl's source-derived convenience categories label scores below `0.50` likely benign and scores at or above `0.50` likely disease causing while advising users to inspect the score and choose a cutoff appropriate to the application.[^revel-ensembl] AnnoCAT therefore uses green below `0.50` and red at or above `0.50`; `0.75` changes the tooltip to note higher specificity but does not change direction or hue. The approved ClinGen calibration takes precedence whenever it applies.

SIFT's developer boundary is strict: scores below `0.05` are deleterious and scores at or above `0.05` are tolerated.[^sift-developer] PolyPhen-2 categories must come from the exact HumDiv or HumVar model because their false-positive-rate thresholds differ.[^polyphen-developer] BayesDel's universal cutoff was designed for gene-discovery research rather than clinical classification, so its tooltip must preserve that limitation.[^bayesdel-developer]

The population-frequency and non-calibrated phyloP cutoffs reproduce Gene.iobio's open-source variant-inspection rules.[^geneiobio-triage] AnnoCAT uses green for the common-frequency band so all three frequency bands remain visually distinct. FAVOR conservation aPC uses FAVOR's documented PHRED-rank landmarks: 10, 20, and 30 correspond to the top 10%, 1%, and 0.1%.[^favor-apc] These are prioritization cues only. Frequency and conservation do not independently classify a variant, and calibrated phyloP pills take precedence when their exact scope applies.

### 6.1 Exact categorical mappings

The current dbNSFP 4.9a category registry permits plain-text color for the following exact fields:

| Fields | Red | Amber | Green | Neutral or informational |
| --- | --- | --- | --- | --- |
| `SIFT_pred`, `SIFT4G_pred`, `PROVEAN_pred`, `MetaSVM_pred`, `MetaLR_pred`, `MetaRNN_pred`, `M-CAP_pred`, `PrimateAI_pred`, `DEOGEN2_pred`, `BayesDel_noAF_pred`, `ClinPred_pred`, `LIST-S2_pred`, `fathmm-XF_coding_pred` | Deleterious or damaging | - | Tolerated or neutral | Unknown code |
| `Polyphen2_HDIV_pred`, `Polyphen2_HVAR_pred` | Probably damaging | Possibly damaging | Benign | Unknown code |
| `MutationTaster_pred` | Disease causing, including automatic | - | Polymorphism, including automatic | Unknown code |
| `MutationAssessor_pred` | High functional impact | Medium functional impact | Neutral | Low functional impact |
| `AlphaMissense_pred` | Likely pathogenic | Uncertain | Likely benign | Unknown code |
| `ESM1b_pred` | Deleterious | - | Tolerated | Unknown code |
| `Aloft_pred` | - | - | Tolerant | Dominant or recessive loss-of-function context |

These labels and codes come from the exact dbNSFP source fields.[^dbnsfp-source] They are display-only and do not become independent PP3 or BP4 votes. `ESM1b_pred` is a dbNSFP-derived category at `-7.5`, not a developer-recommended ESM1b threshold; its tooltip must say `dbNSFP-derived category`. ALoFT `Dominant` and `Recessive` describe loss-of-function inheritance context and remain neutral or informational rather than adverse or reassuring.

Any additional dbNSFP category field requires an explicit mapping before it receives color. Do not implement a generic rule such as "`D` always means damaging." Category codes are field-specific and must be mapped from the exact dbNSFP metadata.

## 7. Prediction summary

### 7.1 Required behavior

The summary must be generated from the same interpreted prediction objects used by the detailed rows. It must not independently reinterpret raw values.

For each predictor family:

1. Select one value for the selected transcript and allele.
2. Resolve a valid calibrated interpretation separately from the direction-count summary.
3. For the direction-count summary, use one verified source category per family. The selected REVEL and verified CADD PHRED scores also contribute one direction each from their existing interpretations.
4. Deduplicate numeric and `_pred` representations of the same predictor.
5. Count the family at most once.
6. Exclude other calibrated alternatives, missing, not applicable, and unrelated neutral measurements from the count. Preserve an indeterminate selected REVEL interpretation or a below-threshold CADD PHRED interpretation as one neutral direction.

The direct-prediction summary may count:

- Red: damaging or pathogenic predictions.
- Amber: uncertain or indeterminate predictions.
- Green: benign or tolerated predictions.

It must not count:

- Allele frequency.
- Conservation shown only as context.
- CADD raw scores, rank scores, or CADD PHRED fields whose identity is not verified.
- Missing values.
- Scores with unverified identities.
- Both a score and category from the same predictor.
- Multiple transcript values from the same predictor.
- Other calibrated alternative predictors as additional votes. REVEL and CADD PHRED are the only numeric predictors included in the direction count.

### 7.2 Separate calibrated evidence from agreement counts

The UI should distinguish:

- `Primary calibrated interpretation`: a red, neutral, or green calibrated pill for the predeclared SpliceAI result when applicable.
- `Prediction directions`: display-only counts of exact source categories plus one selected REVEL direction and one verified CADD PHRED direction, rendered as red, amber, green, or neutral segments.
- `Contextual scores`: individually displayed plain text that does not enter either summary.

The prediction summary must not be labeled `ACMG evidence`, `PP3 count`, `BP4 count`, or `consensus classification`.

### 7.3 Resolve disagreement consistently

The summary and detailed predictions must never disagree because of different thresholds. If a direct category says `deleterious`, the summary cannot call that same predictor `uncertain`.

If a calibrated interpretation and developer category differ:

- The detailed row shows the calibrated interpretation as primary.
- The native category can appear in the tooltip as supplementary context.
- The primary calibrated pill uses the calibrated interpretation.
- The source category from that same predictor family does not also enter `Other prediction directions`.
- The provenance records both values and the decision path.

## 8. Tooltip contract

Tooltips must be concise and structured. The first phrase identifies the interpretation tier:

`[Tier label]: [Predictor] [score] - [interpretation]`

Examples:

- `Calibrated interpretation: REVEL 0.72 - supporting pathogenic range for missense variants`
- `Developer interpretation: SIFT 0.03 - deleterious because the score is below 0.05`
- `Contextual rank: CADD PHRED 21.4 - approximately the top 1 percent; missense calibration not applicable`
- `Directional context: phyloP -2.80 - faster-than-expected evolution; not a pathogenicity classification`
- `Developer interpretation: AlphaMissense 0.51 - uncertain`
- `Neutral measurement: GERP++ RS 1.7 - available constraint score; no non-calibrated direction applied`

Avoid:

- Repeating the field description.
- Long explanations of ACMG/AMP in every tooltip.
- Claiming that a single score classifies the variant.
- Displaying percentile text in the table cell.
- Showing internal field keys unless requested.
- Calling a dbNSFP-derived category a developer category.

The full scientific explanation, release, calibration, and references belong in a help panel or provenance section.

## 9. Provenance contract

Every interpreted computational value must retain:

- Report source ID.
- Source release or version.
- Exact field name.
- Exact score identity.
- Predictor ID and model version, where known.
- Selected allele.
- Selected transcript.
- Selected transcript consequence.
- Calibration ID and version.
- Calibration reference.
- Applicability decision.
- Reason calibration was or was not applied.
- Raw score.
- Source categorical call, when present.
- Final display interpretation.
- Summary inclusion or exclusion reason.

FAVOR online annotations need endpoint-specific handling. FAVOR's
machine-readable coding API contract explicitly identifies its coding CF as
dbNSFP 5.3.1a. The official dbNSFP release history independently confirms that
MisFit first appears in 5.3 and popEVE in 5.3.1, matching the live coding
schema.[^favor-coding-contract][^dbnsfp-releases] The general FAVOR annotation
catalog separately identifies its standard dbNSFP collection as v5.2; that
version must not be assigned to coding-CF fields.

AnnoCAT records the endpoint, fetch time, endpoint contract, and observed schema
fingerprint. That provider contract is sufficient to use a registered
field-specific threshold when the exact predictor field, score identity, model
version, and biological applicability match. It is not sufficient to claim that
a provider-selected transcript summary matches AnnoCAT's selected transcript.

- A verified calibrated mapping may use the calibrated soft pill.
- A verified source-native or display threshold may use plain semantic color.
- A verified returned categorical call may use its field-specific category
  mapping.
- CADD rank context applies only to a verified CADD PHRED field.
- Predictors whose numbered release or score identity remains unknown stay
  neutral unless a verified returned category applies.
- FAVOR response provenance remains separate from the upstream predictor
  identity.

## 10. Current source identities

The current AnnoCAT configuration includes:

- dbNSFP 4.9a.
- CADD v1.7 as a dedicated source.
- REVEL 1.3 as a dedicated source.
- Dedicated SpliceAI data.
- AlphaMissense scores from pinned dbNSFP 4.9a; the separate AlphaMissense catalog entry is only a future adapter placeholder.
- FAVOR standard fields and curated dbNSFP coding fields from the online API.

The exact source and release must still be taken from result provenance at runtime. A configured source version does not prove that every imported or online result field has the same identity.

## 11. Implementation status

Implemented and covered by executable boundary tests:

1. PolyPhen calibration is bound only to dbNSFP `Polyphen2_HVAR_score`; HumDiv numeric scores do not inherit the HumVar calibration.
2. AlphaMissense, VARITY_R, and ESM1b preserve the discrete precision reported by Bergquist Table 1. Values in higher-precision gaps remain uncalibrated.
3. The ESM1b indeterminate endpoint is `-6.4`, with the neighboring one-decimal intervals preserved exactly.
4. Every calibrated pathogenic direction uses the adverse hue, including supporting intervals. Strength remains text and tooltip information.
5. Calibrated values use semantic soft pills; exact source-native and contextual interpretations use plain text.
6. Allele frequency and non-calibrated phyloP use Gene.iobio-style triage bands. FAVOR conservation aPC uses documented PHRED-rank bands. Other aPCs, GERP++, APPRIS, cCRE, available quality metrics, high mappability, and unverified predictor values remain neutral rather than informational blue.
7. Exact predictor source and field identities replace numeric field-name-regex fallbacks.
8. Native SIFT uses `< 0.05` as deleterious and `>= 0.05` as tolerated.
9. Low mappability is amber technical caution; high mappability is neutral without a quality profile.
10. The table, summary grid, detailed evidence rows, and tooltips consume the same structured interpretation.
11. Prediction counts include exact source-native categorical calls plus one selected REVEL interpretation and one verified CADD PHRED interpretation, deduplicated by predictor family. SpliceAI remains a separate primary pill.
12. The calibrated-color setting switches between calibrated pills and exact non-calibrated source-native colors without changing raw values.
13. Native numeric colors cover verified REVEL, BayesDel no-AF, PrimateAI, PolyPhen-2 HVAR, SIFT, AlphaMissense, CADD PHRED, SpliceAI maximum delta, phyloP, and population-frequency fields; unsupported numeric predictors remain neutral.
14. FAVOR MutPred2 is an optional calibrated alternative exposed in Columns and prediction details; it is not a recommended default column or an independent prediction-summary vote alongside REVEL.

Remaining limitations are provenance and profile inputs rather than display-rule fallbacks:

- Imported result evidence does not yet expose an immutable per-field upstream release to the browser. The configured source identity is therefore necessary but not sufficient proof for every imported result.
- ESM1b and the future dedicated AlphaMissense adapter remain unverified and
  neutral until their exact model releases and score identities are registered.
- FAVOR coding fields may use a calibrated pill only when stored fetch
  provenance pins the 5.3.1a endpoint contract and the exact predictor mapping;
  standard FAVOR fields remain bound to their separate catalog contract.
- Sequencing metrics remain neutral until versioned assay- and caller-specific QC profiles exist.
- Filters, sorting, and export continue to operate on raw values; normalized interpretation categories are not yet exposed as separate filter or export fields.

## 12. Runtime implementation contract

### 12.1 Authoritative registry

`config/evidence-calibrations.json` is the runtime source of truth. AnnoCAT
embeds it in `annocat-core`, validates it as part of the source catalog, and
serves it to the viewer through `/api/evidence-calibrations`.

The registry contains the display-only policy, exact predictor identities,
source and field matches, source verification status, consequence
applicability, calibrated bands, source-native bands, categorical mappings,
and contextual display policies. An approved source match must identify a
source version. An unverified match must state why it is unverified.

Predictor-specific thresholds do not belong in table, details, tooltip, or
summary components. A field that has no exact registered identity remains
neutral.

### 12.2 Shared presentation result

`web/src/app/variant-presentation.js` resolves source identity, field identity,
source verification, consequence applicability, calibrated interpretation,
and source-native fallback into one presentation result. Its stable display
properties are:

```text
display
tone
tier
presentation
```

The result can also carry the matched evidence band, native interpretation,
applicability explanation, and concise summary text. The table, Variant
Details, prediction summary, and tooltips consume this shared result. Raw
stored values remain unchanged.

### 12.3 Field-specific category mappings

Every categorical mapping must:

- name one exact source and field;
- enumerate every accepted source code;
- define its display label and semantic tone;
- define unknown-code behavior; and
- include a source reference.

Unknown category codes render as neutral text. They must not default to red,
green, or amber.

## 13. Maintenance rules

1. Update the policy, runtime registry, source citation, and boundary tests in
   the same change.
2. Approve a calibrated source match only when its field, model or release,
   allele, transcript where applicable, and variant type match the published
   calibration.
3. Compare raw values without silent rounding. Preserve published discrete
   precision gaps.
4. Keep canonical splice donor and acceptor variants outside the generic
   SpliceAI calibration.
5. Route table, details, summary, and tooltip presentation through the shared
   presenter.
6. Keep unknown, incompatible, or unresolved evidence neutral.
7. Keep filtering, sorting, and export based on raw stored values unless a
   separate versioned derived-category contract is introduced.
8. Keep gene-specific VCEP rules outside this global policy until AnnoCAT has a
   separately versioned rule source and applicability model.

## 14. Validation checklist

### Scientific behavior

- A REVEL score on a missense selected transcript uses the approved REVEL band.
- The same REVEL score on a non-missense selected transcript does not use missense calibration.
- Calibrated pathogenic intervals use red pills even at supporting strength; calibrated indeterminate intervals use neutral pills.
- CADD uses a calibrated pill only for missense.
- Exact non-missense CADD PHRED uses neutral below 10, amber from 10 to below 20, and red at 20 or above, with rank context in its tooltip.
- SIFT direction is correctly inverted: lower scores are more damaging.
- Native SIFT treats exactly `0.05` as tolerated.
- Pejaver calibration is applied only to the exact PolyPhen-2 HumVar score; HumDiv remains a separate source-native interpretation.
- SpliceAI generic calibration is not applied to canonical donor or acceptor variants.
- Native SpliceAI below `0.20` remains neutral rather than green.
- Conservation is not treated as an independent variant classification.
- Non-calibrated phyloP and population frequency use the documented Gene.iobio triage bands; uncalibrated GERP++ remains neutral.
- Missing FAVOR values do not render as zero.
- Unverified ESM1b and legacy MutPred v1.2 fields remain contextual.

### Summary behavior

- A predictor appears no more than once.
- Numeric and category fields from one predictor are deduplicated.
- The selected transcript determines missense applicability.
- Summary colors exactly match the detailed predictor rows.
- Directional-rank context and neutral measurements are excluded from counts except for the verified CADD PHRED family.
- The primary calibrated REVEL or SpliceAI interpretation is shown as a pill separate from direction counts.
- Correlated missense predictors are not stacked into ACMG evidence.

### UI behavior

- Red, green, amber, neutral, blue-interactive, and gray have consistent meanings everywhere.
- Pill shape identifies calibrated interpretation; plain text identifies non-calibrated interpretation.
- A neutral calibrated pill is visibly different from gray missing text.
- Blue is not used as the default evidence color.
- Color is never the only carrier of meaning.
- Tooltips remain one concise sentence.
- The table uses compact values without percentile suffixes.
- Details show source and applicability on demand.
- Filters use raw values plus the same normalized interpretation categories.
- Ordinary-size colored text meets 4.5:1 contrast on its background.
- Pill boundaries and other meaningful non-text indicators meet 3:1 contrast against adjacent colors.
- Predictor interpretation colors and sequencing-quality colors remain separate domains.
- Combined quality cells color FILTER, QUAL, DP, GQ, and allelic support independently.
- `PASS` does not hide poor depth, low GQ, or imbalanced allelic support.
- Missing quality values remain gray and are never converted to zero or pass.
- An aggregate QC status reflects the worst applicable metric and is excluded from the prediction summary.

### Boundary tests

Every interval boundary needs tests immediately below, exactly at, and immediately above the threshold. This is required for all inclusive and exclusive boundaries, especially:

- REVEL `0.290` and `0.644`.
- SIFT `0.000`, `0.001`, `0.080`, and `0.327`.
- CADD `17.3`, `22.7`, `25.3`, and `28.1`.
- SpliceAI `0.10` and `0.20`.
- AlphaMissense `0.070`, `0.099`, `0.169`, `0.792`, `0.906`, `0.972`, and `0.990`.
- VARITY_R `0.036`, `0.063`, `0.116`, `0.251`, `0.675`, `0.842`, `0.915`, and `0.965`.
- ESM1b `8.8`, `8.7`, `-3.1`, `-3.2`, `-6.3`, `-6.4`, `-10.6`, `-10.7`, `-12.1`, `-12.2`, `-13.9`, `-14.0`, `-23.9`, and `-24.0`.

For the 2025 tables, tests must also use higher-precision values in the gaps between displayed endpoints, such as AlphaMissense `0.0705`, VARITY_R `0.0365`, and ESM1b `-6.35`. Those values remain uncalibrated unless the matched source has a verified rounding contract.

Non-calibrated display mappings also need boundary tests:

- CADD PHRED `10`, `20`, and `30`.
- SIFT `0.05`.
- REVEL `0.50` and `0.75`.
- BayesDel no-AF `-0.0570105`.
- PrimateAI `0.60` and `0.80`.
- PolyPhen-2 HVAR `0.446` and `0.908`.
- SpliceAI `0.20`, `0.50`, and `0.80`.
- AlphaMissense `0.34` and `0.564`.
- phyloP `0`.

## 15. References

Primary and authoritative sources:

- [ClinGen SVI: Calibration of computational tools for missense variant pathogenicity classification](https://clinicalgenome.org/docs/calibration-of-computational-tools-for-missense-variant-pathogenicity-classification-and-clingen-recommendations-for-pp3-bp4-cri/)
- [Pejaver et al. 2022, full text and Table 2](https://pmc.ncbi.nlm.nih.gov/articles/PMC9748256/#tbl2)
- [Bergquist et al. 2025, final full text and Table 1](https://pmc.ncbi.nlm.nih.gov/articles/PMC12208618/#tbl1)
- [Walker et al. 2023, full text](https://pmc.ncbi.nlm.nih.gov/articles/PMC10357475/)
- [ClinGen SVI splicing subgroup response to feedback](https://clinicalgenome.org/docs/clingen-svi-splicing-subgroup-response-to-feedback/)
- [ClinGen variant classification guidance](https://clinicalgenome.org/tools/clingen-variant-classification-guidance/)
- [CADD score interpretation](https://cadd.bihealth.org/info)
- [SpliceAI repository and developer guidance](https://github.com/Illumina/SpliceAI)
- [AlphaMissense developer interpretation](https://www.ebi.ac.uk/training/online/courses/alphafold/classifying-the-effects-of-missense-variants-using-alphamissense/)
- [REVEL original publication](https://pmc.ncbi.nlm.nih.gov/articles/PMC5065685/)
- [Hopkins et al. 2023, REVEL author-threshold evaluation](https://pmc.ncbi.nlm.nih.gov/articles/PMC11918948/)
- [Ensembl pathogenicity-prediction categories](https://grch37.ensembl.org/info/genome/variation/prediction/protein_function.html)
- [BayesDel first-party score guidance](https://fenglab.chpc.utah.edu/BayesDel.html)
- [PrimateAI original publication](https://pmc.ncbi.nlm.nih.gov/articles/PMC6237276/)
- [SIFT missense predictions for genomes](https://sift.bii.a-star.edu.sg/www/nprot2016_vaser.pdf)
- [PolyPhen-2 model and category documentation](https://genetics.bwh.harvard.edu/wiki/%21pph2/overview)
- [UCSC phyloP definition](https://genome.ucsc.edu/docs/genomeBrowserGlossary.html)
- [GERP++ constraint definition](https://pmc.ncbi.nlm.nih.gov/articles/PMC2996323/)
- [UCSC GERP track interpretation](https://genome.ucsc.edu/cgi-bin/hgTrackUi?g=allHg19RS_BW)
- [dbNSFP](https://www.dbnsfp.org/)
- [dbNSFP source and release page](https://sites.google.com/site/jpopgen/dbNSFP)
- [dbNSFP 4.9a field reference](https://grr.iossifovlab.com/hg38/scores/dbNSFP4.9a/index.html)
- [Ensembl calculated variant consequences](https://www.ensembl.org/info/genome/variation/prediction/predicted_data.html)
- [GATK VCF field documentation](https://gatk.broadinstitute.org/hc/en-us/articles/360035531692-VCF-Variant-Call-Format)
- [GATK allele-depth documentation](https://gatk.broadinstitute.org/hc/en-us/articles/360037421851-DepthPerAlleleBySample)
- [ACMG technical standard for constitutional NGS, 2021 revision](https://www.gimjournal.org/article/S1098-3600(21)05056-5/fulltext)
- [ClinGen SVI recommendation for absence and rarity](https://clinicalgenome.org/site/assets/files/5756/variant_classification_using_acmg_amp_interpreting_sequence_guidelines_harrison.pdf)
- [WCAG 2.1 use of color](https://www.w3.org/WAI/WCAG21/Understanding/use-of-color.html)
- [WCAG 2.1 contrast minimum](https://www.w3.org/WAI/WCAG21/Understanding/contrast-minimum)
- [WCAG 2.1 non-text contrast](https://www.w3.org/WAI/WCAG21/Understanding/non-text-contrast.html)
- [Fluent 2 color guidance](https://fluent2.microsoft.design/color)
- [Fluent 2 color tokens](https://fluent2.microsoft.design/color-tokens/)
- [Fluent 2 badge guidance](https://fluent2.microsoft.design/components/web/react/core/badge/usage)

[^pejaver-2022-table2]: [Pejaver et al. 2022, Table 2](https://pmc.ncbi.nlm.nih.gov/articles/PMC9748256/#tbl2), "Estimated threshold ranges for all tools in this study corresponding to the four pathogenic and four benign intervals." The methods identify the evaluated PolyPhen-2 score as the HumVar model.

[^pejaver-2022-table4]: [Pejaver et al. 2022, Table 4](https://pmc.ncbi.nlm.nih.gov/articles/PMC9748256/#tbl4), which recommends a calibrated single tool selected before viewing its score rather than an uncalibrated consensus.

[^bergquist-2025-table1]: [Bergquist et al. 2025, Table 1](https://pmc.ncbi.nlm.nih.gov/articles/PMC12208618/#tbl1), which reports discrete AlphaMissense and VARITY_R intervals to three decimal places, ESM1b intervals to one decimal place, and additional 3-point intervals for a future point-based framework. [PubMed PMID 40084623](https://pubmed.ncbi.nlm.nih.gov/40084623/), the [final Genetics in Medicine article](https://www.gimjournal.org/article/S1098-3600%2825%2900049-8/fulltext), and [DOI 10.1016/j.gim.2025.101402](https://doi.org/10.1016/j.gim.2025.101402) identify the final publication.

[^walker-2023]: [Walker et al. 2023](https://pmc.ncbi.nlm.nih.gov/articles/PMC10357475/), ClinGen SVI recommendations for predicted and observed splicing impact.

[^clingen-splicing-feedback]: [ClinGen SVI splicing subgroup response to feedback](https://clinicalgenome.org/docs/clingen-svi-splicing-subgroup-response-to-feedback/), confirming generic use of SpliceAI `>= 0.2` for PP3 and `<= 0.1` for BP4 while noting that transcript annotation and run settings can change scores.

[^clingen-current-guidance]: [ClinGen variant classification guidance](https://clinicalgenome.org/tools/clingen-variant-classification-guidance/), the current official index of ClinGen-endorsed ACMG/AMP criteria recommendations and point-system guidance reviewed on 2026-07-26.

[^cadd-interpretation]: [CADD score interpretation](https://cadd.bihealth.org/info), including the PHRED rank meaning and the absence of a natural universal deleteriousness cutoff.

[^spliceai-developer]: [Illumina SpliceAI developer guidance](https://github.com/Illumina/SpliceAI), documenting maximum delta score, the `0.20` high-recall, `0.50` recommended, and `0.80` high-precision cutoffs, and the effect of raw versus masked score configuration.

[^alphamissense-developer]: [EMBL-EBI AlphaMissense score guidance](https://www.ebi.ac.uk/training/online/courses/alphafold/classifying-the-effects-of-missense-variants-using-alphamissense/understanding-pathogenicity-scores-from-alphamissense/), documenting the developer categories below `0.34`, from `0.34` through `0.564`, and above `0.564`.

[^sift-developer]: [Vaser et al. 2016, SIFT missense predictions for genomes](https://sift.bii.a-star.edu.sg/www/nprot2016_vaser.pdf), Table 3, defining scores below `0.05` as deleterious and scores at or above `0.05` as tolerated.

[^polyphen-developer]: [PolyPhen-2 model and category documentation](https://genetics.bwh.harvard.edu/wiki/%21pph2/overview), documenting benign, possibly damaging, and probably damaging categories and different false-positive-rate thresholds for HumDiv and HumVar.

[^revel-developer]: [Ioannidis et al. 2016, REVEL original publication](https://pmc.ncbi.nlm.nih.gov/articles/PMC5065685/), together with [Hopkins et al. 2023](https://pmc.ncbi.nlm.nih.gov/articles/PMC11918948/), which reports the REVEL authors' proposed `0.50` sensitivity-oriented and `0.75` specificity-oriented pathogenicity thresholds and their tradeoff.

[^revel-ensembl]: [Ensembl pathogenicity-prediction categories](https://grch37.ensembl.org/info/genome/variation/prediction/protein_function.html), which labels REVEL scores below `0.50` likely benign and scores at or above `0.50` likely disease causing as a convenience display while recommending use of the actual score.

[^bayesdel-developer]: [BayesDel first-party score guidance](https://fenglab.chpc.utah.edu/BayesDel.html), which defines the no-AF universal cutoff as `-0.0570105`, states that higher scores are more likely pathogenic, and warns that the cutoff was designed for gene-discovery research rather than clinical operations.

[^primateai-developer]: [Sundaram et al. 2018, PrimateAI original publication](https://pmc.ncbi.nlm.nih.gov/articles/PMC6237276/), which recommends `< 0.60` as likely benign, `0.60-0.80` as intermediate, and `> 0.80` as likely pathogenic.

[^phylop-semantics]: [UCSC Genome Browser glossary](https://genome.ucsc.edu/docs/genomeBrowserGlossary.html) and [PHAST phyloP documentation](https://manpages.debian.org/unstable/phast/phyloP.1.en.html), defining positive scores as conservation and negative scores as faster-than-expected evolution or acceleration in CONACC mode.
[^geneiobio-triage]: Gene.iobio source at commit [`f35833d`](https://github.com/iobio/gene.iobio/tree/f35833d2c010233fd08ccf1f85a28b822e74d80f): [allele-frequency bands](https://github.com/iobio/gene.iobio/blob/f35833d2c010233fd08ccf1f85a28b822e74d80f/client/app/components/viz/VariantInspectCard.vue#L1528-L1535) and [phyloP conservation bands](https://github.com/iobio/gene.iobio/blob/f35833d2c010233fd08ccf1f85a28b822e74d80f/client/app/components/viz/VariantInspectCard.vue#L1557-L1588).

[^favor-apc]: [FAVOR annotation principal-component documentation](https://favor-beta.genohub.org/docs/data#annotation-principal-components-apcs), which defines conservation aPC as a first-PC summary of GERP, PhastCons, and phyloP fields and defines PHRED 10, 20, and 30 as the top 10%, 1%, and 0.1% of the genome-wide rank distribution.

[^gerp-semantics]: [Davydov et al. 2010](https://pmc.ncbi.nlm.nih.gov/articles/PMC2996323/), defining positive GERP++ RS values as substitution deficits associated with constraint, together with the [UCSC GERP track guidance](https://genome.ucsc.edu/cgi-bin/hgTrackUi?g=allHg19RS_BW), which cautions against interpreting negative values as accelerated evolution because of confounders.

[^dbnsfp-source]: [dbNSFP source and release documentation](https://www.dbnsfp.org/releases/), [dbNSFP v4 publication](https://pmc.ncbi.nlm.nih.gov/articles/PMC7709417/), and the [dbNSFP 4.9a field reference](https://grr.iossifovlab.com/hg38/scores/dbNSFP4.9a/index.html).

[^favor-coding-contract]: FAVOR's machine-readable [API reference](https://api-v2.genohub.org/docs) describes `/variants/batch/coding` as a lookup of the coding-CF record from dbNSFP 5.3.1a.

[^dbnsfp-releases]: The official [dbNSFP release history](https://www.dbnsfp.org/releases/) states that v5.3 introduced MisFit and v5.3.1 added popEVE, and publishes separate 5.3.1a and 5.3.1c READMEs.

[^ensembl-consequences]: [Ensembl calculated variant consequences](https://www.ensembl.org/info/genome/variation/prediction/predicted_data.html), which defines the consequence severity order and assigns `upstream_gene_variant` to the MODIFIER impact class.

[^clingen-pm2]: [ClinGen SVI recommendation for absence and rarity](https://clinicalgenome.org/site/assets/files/5756/variant_classification_using_acmg_amp_interpreting_sequence_guidelines_harrison.pdf), which reduces absence or rarity evidence to supporting strength and requires population-frequency interpretation in the relevant disease context.

[^acmg-ngs-2021]: [Rehm et al. 2021, ACMG technical standard for constitutional NGS](https://www.gimjournal.org/article/S1098-3600(21)05056-5/fulltext), which explains that minimum coverage and allele-fraction ranges must be established for the assay and notes that suggested minimum values are not sufficient for every variant or region.

[^gatk-vcf-fields]: [GATK VCF field documentation](https://gatk.broadinstitute.org/hc/en-us/articles/360035531692-VCF-Variant-Call-Format), defining FILTER, QUAL, genotype, GQ, DP, and AD and explaining the distinction between site and sample confidence.

[^gatk-ad]: [GATK allele-depth documentation](https://gatk.broadinstitute.org/hc/en-us/articles/360037421851-DepthPerAlleleBySample), defining AD order as REF followed by ALT alleles and cautioning against using AD alone to infer the associated genotype.

[^wcag-color]: [WCAG 2.1 Understanding 1.4.1](https://www.w3.org/WAI/WCAG21/Understanding/use-of-color.html), requiring that color not be the only visual means of conveying information.

[^wcag-contrast]: [WCAG 2.1 Understanding 1.4.3](https://www.w3.org/WAI/WCAG21/Understanding/contrast-minimum) and [Understanding 1.4.11](https://www.w3.org/WAI/WCAG21/Understanding/non-text-contrast.html), requiring at least 4.5:1 contrast for ordinary text and 3:1 for meaningful non-text UI indicators.

[^fluent-color]: [Fluent 2 color guidance](https://fluent2.microsoft.design/color), defining status colors as semantic feedback colors, recommending sparse and consistent use, and reserving brand color for product identity and interaction.

[^fluent-tokens]: [Fluent 2 web alias color tokens](https://fluent2.microsoft.design/color-tokens/), defining status foreground, background, and border aliases for danger, warning, and success across light and dark themes.

[^fluent-badge]: [Fluent 2 badge guidance](https://fluent2.microsoft.design/components/web/react/core/badge/usage), recommending short status badges, intentional semantic color, consistent sizing within a context, and accessible text equivalents.

## 16. Open verification items

These must be resolved before implementation is considered scientifically complete:

1. Verify dbNSFP 4.9a ESM1b score precision and selected-transcript mapping against the 2025 calibration input.
2. Verify source precision and rounding contracts before applying the discrete 2025 AlphaMissense, VARITY_R, or ESM1b intervals to higher-precision runtime values.
3. Persist the FAVOR coding endpoint contract with each fetch and register exact
   model releases for coding predictors whose numbered release is not identified
   by dbNSFP 5.3.1a before enabling their calibrated colors.
4. Pin a release contract before enabling the separate AlphaMissense adapter.
5. Define and version the initial WGS, WES, panel, mitochondrial, mosaic, and somatic sequencing-quality profiles before enabling threshold-based QC colors.

Gene-specific VCEP overrides are intentionally outside the current scope. The active policy is global-only calibration.

Resolved in the registry:

- dbNSFP 4.9a `MutPred_score` is identified as legacy MutPred v1.2 and cannot use MutPred2 calibration.
- FAVOR coding `mutpred2.score` and its dbNSFP calibrated category are registered against the Pejaver 2022 MutPred2 intervals.
- The FAVOR coding endpoint is verified as dbNSFP 5.3.1a; the standard FAVOR
  annotation catalog remains separately versioned as dbNSFP 5.2.
- dbNSFP 5.3.1a predictor identities and available releases are recorded for
  curated FAVOR coding fields; missing numbered predictor releases remain
  explicit.
- Every curated dbNSFP `_pred` field has an explicit source-native mapping.
- Published interval notation and source precision are preserved; higher-precision gaps are not silently interpolated.
- Hue encodes direction; calibrated strength is text and tooltip information, not saturation.
- The 2025 intermediate intervals are labeled `3-point`, not `moderate-to-strong`.

Until those items are resolved, a field with an exact verified identity but incompatible calibration metadata may remain Tier B or Tier C. A field whose score identity itself is unresolved is Tier D neutral measurement. Neither case can be presented as calibrated evidence.
