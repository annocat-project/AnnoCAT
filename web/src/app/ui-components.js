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

export function requestFluentText({
  title,
  label,
  value = '',
  confirmLabel = 'Save',
  maxLength = 80,
}) {
  let dialog = document.querySelector('#fui-text-input-dialog');
  if (!dialog) {
    document.body.insertAdjacentHTML('beforeend', `
      <dialog id="fui-text-input-dialog" class="fui-dialog" aria-labelledby="fui-text-input-title">
        <form method="dialog">
          <header class="fui-dialog__header"><h2 id="fui-text-input-title"></h2></header>
          <div class="fui-dialog__content">
            <label class="fui-field"><span class="fui-field__label" data-fui-text-input-label></span><input class="fui-input" data-fui-text-input required></label>
          </div>
          <footer class="fui-dialog__footer">
            <button type="submit" value="cancel" class="fui-button">Cancel</button>
            <button type="submit" value="confirm" class="fui-button fui-button--primary" data-fui-text-input-confirm>Save</button>
          </footer>
        </form>
      </dialog>`);
    dialog = document.querySelector('#fui-text-input-dialog');
    const input = dialog.querySelector('[data-fui-text-input]');
    input.addEventListener('input', () => {
      dialog.querySelector('[data-fui-text-input-confirm]').disabled = !input.value.trim();
    });
  }
  const input = dialog.querySelector('[data-fui-text-input]');
  dialog.querySelector('#fui-text-input-title').textContent = title;
  dialog.querySelector('[data-fui-text-input-label]').textContent = label;
  dialog.querySelector('[data-fui-text-input-confirm]').textContent = confirmLabel;
  input.maxLength = maxLength;
  input.value = value;
  dialog.querySelector('[data-fui-text-input-confirm]').disabled = !value.trim();
  dialog.returnValue = '';
  return new Promise(resolve => {
    dialog.addEventListener('close', () => {
      resolve(dialog.returnValue === 'confirm' ? input.value.trim() : null);
    }, { once: true });
    openFluentDialog(dialog, input);
    requestAnimationFrame(() => input.select());
  });
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
