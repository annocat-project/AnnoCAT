export function createNaturalResultOrder(rows) {
  return new Map(rows.map((row, index) => [row.alleleId, {
    recordNumber: finiteNumber(row.recordNumber),
    altIndex: finiteNumber(row.altIndex),
    index,
  }]));
}

export function compareNaturalResultOrder(order, left, right) {
  const leftOrder = order.get(left.alleleId);
  const rightOrder = order.get(right.alleleId);
  if (!leftOrder || !rightOrder) return leftOrder ? -1 : rightOrder ? 1 : 0;
  return compareNumber(leftOrder.recordNumber, rightOrder.recordNumber)
    || compareNumber(leftOrder.altIndex, rightOrder.altIndex)
    || leftOrder.index - rightOrder.index;
}

function finiteNumber(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : Number.MAX_SAFE_INTEGER;
}

function compareNumber(left, right) {
  return left === right ? 0 : left < right ? -1 : 1;
}
