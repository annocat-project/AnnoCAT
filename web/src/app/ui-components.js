/*
 * Shared behavior for the explicit fui-* component API.
 *
 * Component anatomy and styling live in fluent-components.css. Markup is
 * responsible for declaring its component classes directly.
 */

export function installFluentComponentSystem(root = document) {
  root.documentElement?.classList.add('fui-components-ready');
  root.addEventListener('keydown', event => {
    if (event.key === 'Tab') root.documentElement?.classList.add('fui-keyboard-navigation');
  }, true);
  root.addEventListener('pointerdown', () => {
    root.documentElement?.classList.remove('fui-keyboard-navigation');
  }, true);
  const violations = findUnclassifiedInteractiveElements(root);
  root.documentElement?.toggleAttribute('data-fui-contract-violations', violations.length > 0);
  if (violations.length) {
    console.warn('AnnoCAT component contract violations:', violations);
  }
}

export function findUnclassifiedInteractiveElements(root = document) {
  const componentSelector = [
    '.fui-button',
    '.fui-card--interactive',
    '.fui-menu-item',
    '.fui-nav-item',
    '.fui-tab',
    '.fui-data-grid__sort-button',
    '.fui-data-grid__icon-button',
    '.fui-input',
    '.fui-select',
    '.fui-textarea',
    '.fui-checkbox',
    '.fui-radio',
  ].join(',');
  return [...root.querySelectorAll('button,input:not([type="hidden"]):not([hidden]),select,textarea')]
    .filter(element => !element.matches(componentSelector));
}

export function openFluentDialog(dialog, focusTarget) {
  if (!dialog || dialog.open) return;
  const target = focusTarget || dialog;
  const temporaryAutofocus = !target.hasAttribute('autofocus');
  if (temporaryAutofocus) target.setAttribute('autofocus', '');
  dialog.showModal();
  if (temporaryAutofocus) target.removeAttribute('autofocus');
  dialog.scrollTop = 0;
  dialog.querySelectorAll('.fui-dialog__content--scrollable').forEach(region => {
    region.scrollTop = 0;
  });
  requestAnimationFrame(() => target.focus?.({ preventScroll: true }));
}

export function retainFluentModalFocus(modal, event) {
  if (!modal || event.key !== 'Tab') return;
  const focusable = [...modal.querySelectorAll(
    'button:not(:disabled),input:not([type="hidden"]):not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex]:not([tabindex="-1"])'
  )].filter(element => element.getClientRects().length > 0);
  if (!focusable.length) {
    event.preventDefault();
    modal.focus?.({ preventScroll: true });
    return;
  }
  const first = focusable[0];
  const last = focusable.at(-1);
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
