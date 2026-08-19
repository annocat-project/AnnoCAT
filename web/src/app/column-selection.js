export const MAX_VISIBLE_EVIDENCE_COLUMNS = 32;

export function shouldSelectColumnGroup(fields, evidenceSelectionCount, maxEvidenceColumns = MAX_VISIBLE_EVIDENCE_COLUMNS) {
  const selectedCount = fields.filter(field => field.selected).length;
  const evidenceLimitReached = evidenceSelectionCount >= maxEvidenceColumns
    && fields.some(field => field.evidence && !field.selected);

  return selectedCount < fields.length && !(selectedCount > 0 && evidenceLimitReached);
}
