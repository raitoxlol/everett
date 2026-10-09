const search = document.querySelector('#search');
const harness = document.querySelector('#harness-filter');
const state = document.querySelector('#state-filter');
const rows = [...document.querySelectorAll('[data-session]')];
const visibleCount = document.querySelector('#visible-count');
const filteredEmpty = document.querySelector('#filtered-empty');

function syncNavigation() {
  const target = location.hash && location.hash !== '#top' ? location.hash : '#sessions';
  document.querySelectorAll('nav a').forEach((link) => {
    const selected = link.getAttribute('href') === target;
    link.classList.toggle('active', selected);
    if (selected) link.setAttribute('aria-current', 'location');
    else link.removeAttribute('aria-current');
  });
}
window.addEventListener('hashchange', syncNavigation);
syncNavigation();

function applyFilters() {
  const previousPositions = new Map(rows.map((row) => [row, row.getBoundingClientRect().top]));
  const query = search.value.trim().toLowerCase();
  let visible = 0;
  rows.forEach((row) => {
    const matches = (!query || row.dataset.search.includes(query))
      && (!harness.value || row.dataset.harness === harness.value)
      && (!state.value || row.dataset.state === state.value);
    row.hidden = !matches;
    if (matches) visible += 1;
  });
  visibleCount.textContent = `${visible} shown`;
  filteredEmpty.hidden = visible !== 0 || rows.length === 0;
  if (!matchMedia('(prefers-reduced-motion: reduce)').matches) {
    rows.filter((row) => !row.hidden).forEach((row) => {
      const distance = previousPositions.get(row) - row.getBoundingClientRect().top;
      row.querySelector('.route-stop span').animate([
        { transform: `translateY(${distance}px)` },
        { transform: 'translateY(0)' },
      ], { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' });
    });
  }
}

[search, harness, state].forEach((control) => control?.addEventListener('input', applyFilters));
document.querySelector('#reset-filters')?.addEventListener('click', () => {
  search.value = '';
  harness.value = '';
  state.value = '';
  applyFilters();
  search.focus();
});

document.addEventListener('toggle', (event) => {
  if (event.target.tagName !== 'DETAILS' || !event.target.open) return;
  document.querySelectorAll('.session-row details[open]').forEach((details) => {
    if (details !== event.target) details.removeAttribute('open');
  });
}, true);

document.querySelectorAll('[data-copy]').forEach((button) => {
  const label = button.textContent;
  button.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      button.textContent = 'Copied';
      setTimeout(() => { button.textContent = label; }, 1400);
    } catch {
      button.textContent = 'Copy failed';
      setTimeout(() => { button.textContent = label; }, 1400);
    }
  });
});
