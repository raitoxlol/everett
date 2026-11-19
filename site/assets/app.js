// Everett docs: navigation, search, copy buttons, TOC scroll-spy, pixel sparks.
(() => {
  const root = document.body.dataset.root || './';
  const still = matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ---- navigation panel (narrow screens)
  const navToggle = document.querySelector('[data-nav-toggle]');
  const setNav = (open) => {
    document.body.classList.toggle('nav-open', open);
    if (navToggle) { navToggle.setAttribute('aria-expanded', String(open)); navToggle.textContent = open ? 'close' : 'menu'; }
  };
  navToggle?.addEventListener('click', () => setNav(!document.body.classList.contains('nav-open')));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && document.body.classList.contains('nav-open')) { setNav(false); navToggle?.focus(); } });
  document.querySelector('.side')?.addEventListener('click', (e) => { if (e.target.closest('a')) setNav(false); });
  const cur = document.querySelector('.side a[aria-current]');
  if (cur) { const side = cur.closest('.side'); side.scrollTop = cur.offsetTop - side.clientHeight / 2; }

  // ---- copy buttons: code blocks, home commands, whole page as Markdown
  const flash = (el, label, ok = true) => {
    const prev = el.dataset.label || el.textContent;
    el.dataset.label = prev;
    el.textContent = label;
    el.classList.toggle('ok', ok);
    setTimeout(() => { el.textContent = prev; el.classList.remove('ok'); }, 1600);
  };
  const copyText = async (text) => {
    if (!navigator.clipboard) return false;
    try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
  };
  document.addEventListener('click', async (e) => {
    const btn = e.target.closest('[data-copy], [data-copy-text], [data-copy-page]');
    if (!btn) return;
    let text = btn.dataset.copyText;
    if (btn.hasAttribute('data-copy')) text = btn.closest('.code')?.querySelector('pre code')?.innerText.replace(/^\$ /gm, '');
    if (btn.hasAttribute('data-copy-page')) {
      try { const r = await fetch(btn.dataset.copyPage); if (!r.ok) throw 0; text = await r.text(); } catch { flash(btn, 'copy failed', false); return; }
    }
    if (text == null) return;
    if (await copyText(text)) flash(btn, 'copied'); else flash(btn, 'select text', false);
  });

  // ---- TOC scroll-spy
  const tocLinks = [...document.querySelectorAll('.toc a')];
  if (tocLinks.length && 'IntersectionObserver' in window) {
    const map = new Map(tocLinks.map((a) => [decodeURIComponent(a.hash.slice(1)), a]));
    const heads = [...map.keys()].map((id) => document.getElementById(id)).filter(Boolean);
    const visible = new Set();
    const mark = () => {
      let current = heads.find((h) => visible.has(h));
      if (!current) { const above = heads.filter((h) => h.getBoundingClientRect().top < 140); current = above[above.length - 1] || heads[0]; }
      tocLinks.forEach((a) => a.classList.toggle('active', map.get(current?.id) === a));
    };
    const io = new IntersectionObserver((entries) => {
      entries.forEach((en) => (en.isIntersecting ? visible.add(en.target) : visible.delete(en.target)));
      mark();
    }, { rootMargin: '-64px 0px -65% 0px' });
    heads.forEach((h) => io.observe(h));
  }

  // ---- pixel sparks (from raitox.tech/everett)
  const COLORS = ['#FF5A1F', '#F596AA', '#EDE6DC'];
  function burst(x, y, n, reach) {
    if (still || !document.body.animate) return;
    x += scrollX; y += scrollY;
    for (let k = 0; k < n; k++) {
      const s = document.createElement('i');
      s.className = 'spark';
      s.style.background = COLORS[k % 3];
      document.body.appendChild(s);
      const a = Math.random() * Math.PI * 2;
      const v = reach * (0.5 + Math.random() * 0.7);
      const dx = Math.round(Math.cos(a) * v / 4) * 4;
      const dy = Math.round((Math.sin(a) * v - reach * 0.3) / 4) * 4;
      s.animate([
        { transform: `translate(${x - 2}px,${y - 2}px)`, opacity: 1 },
        { transform: `translate(${x - 2 + dx}px,${y - 2 + dy}px)`, opacity: 1, offset: 0.6 },
        { transform: `translate(${x - 2 + dx}px,${y - 2 + dy + reach * 0.5}px)`, opacity: 0 },
      ], { duration: 650 + Math.random() * 250, easing: 'steps(8, end)' }).onfinish = () => s.remove();
    }
  }
  document.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    const ic = e.target.closest?.('.pi');
    if (ic) {
      const r = ic.getBoundingClientRect();
      ic.classList.remove('spin'); void ic.getBoundingClientRect(); ic.classList.add('spin');
      burst(r.left + r.width / 2, r.top + r.height / 2, 14, 56);
    } else if (e.pointerType === 'mouse' && !e.target.closest('input, pre, .prose')) {
      burst(e.clientX, e.clientY, 6, 26);
    }
  });
  document.addEventListener('animationend', (e) => { if (e.target.classList?.contains('spin')) e.target.classList.remove('spin'); }, true);

  // ---- search
  const input = document.getElementById('search');
  const box = document.getElementById('search-results');
  if (!input || !box) return;
  let index = null;
  let loading = null;
  let results = [];
  let sel = -1;
  const load = () => loading || (loading = fetch(`${root}search.json`).then((r) => r.json()).then((d) => {
    index = d.map((e) => ({ ...e, lt: e.t.toLowerCase(), lh: e.h.toLowerCase(), lx: e.x.toLowerCase() }));
  }).catch(() => { index = []; }));
  const escHtml = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const highlight = (text, terms) => {
    let out = escHtml(text);
    terms.forEach((t) => { if (t.length > 1) out = out.replace(new RegExp(`(${t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'ig'), '<mark>$1</mark>'); });
    return out;
  };
  const snippet = (x, terms) => {
    const lx = x.toLowerCase();
    const pos = Math.min(...terms.map((t) => lx.indexOf(t)).filter((p) => p >= 0), Infinity);
    if (!isFinite(pos) || pos < 60) return x.slice(0, 160);
    return `…${x.slice(pos - 50, pos + 120)}`;
  };
  const search = (q) => {
    const terms = q.toLowerCase().split(/\s+/).filter(Boolean);
    if (!terms.length || !index) return [];
    const scored = [];
    for (const e of index) {
      let score = 0;
      let all = true;
      for (const t of terms) {
        let s = 0;
        if (e.lt === t) s += 40; else if (e.lt.includes(t)) s += 18;
        if (e.lh === t) s += 30; else if (e.lh.includes(t)) s += 14;
        if (e.lx.includes(t)) s += 3 + Math.min(4, e.lx.split(t).length - 1);
        if (!s) { all = false; break; }
        score += s;
      }
      if (all) scored.push([score + (e.h ? 0 : 2), e]);
    }
    scored.sort((a, b) => b[0] - a[0]);
    return scored.slice(0, 12).map(([, e]) => e);
  };
  const close = () => { box.hidden = true; input.setAttribute('aria-expanded', 'false'); input.removeAttribute('aria-activedescendant'); };
  const render = () => {
    const q = input.value.trim();
    if (!q) { close(); return; }
    results = search(q);
    sel = results.length ? 0 : -1;
    const terms = q.toLowerCase().split(/\s+/).filter(Boolean);
    box.innerHTML = results.length ? results.map((r, i) => `
      <a id="sr-${i}" role="option" href="${root}${r.u}"${i === sel ? ' aria-selected="true"' : ''}>
        <span class="r-path">${escHtml(r.g.toLowerCase())} / ${escHtml(r.t)}</span>
        <span class="r-title">${highlight(r.h || r.t, terms)}</span>
        <span class="r-text">${highlight(snippet(r.x, terms), terms)}</span>
      </a>`).join('') : `<p class="search-empty">Nothing for “${escHtml(q)}”. Try a command such as <code>send</code> or a tool such as <code>everett_inbox</code>.</p>`;
    box.hidden = false;
    input.setAttribute('aria-expanded', 'true');
    if (sel >= 0) input.setAttribute('aria-activedescendant', `sr-${sel}`); else input.removeAttribute('aria-activedescendant');
  };
  const move = (d) => {
    if (!results.length) return;
    sel = (sel + d + results.length) % results.length;
    box.querySelectorAll('[role="option"]').forEach((a, i) => a.setAttribute('aria-selected', String(i === sel)));
    document.getElementById(`sr-${sel}`)?.scrollIntoView({ block: 'nearest' });
    input.setAttribute('aria-activedescendant', `sr-${sel}`);
  };
  input.addEventListener('focus', load);
  input.addEventListener('input', () => { if (index) render(); else load().then(render); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
    else if (e.key === 'Enter' && sel >= 0) { e.preventDefault(); location.href = `${root}${results[sel].u}`; close(); }
    else if (e.key === 'Escape') { if (!box.hidden) { e.stopPropagation(); close(); } else input.blur(); }
  });
  document.addEventListener('click', (e) => { if (!e.target.closest('.search')) close(); });
  document.addEventListener('keydown', (e) => {
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName) || document.activeElement?.isContentEditable;
    if ((e.key === '/' && !typing) || ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k')) { e.preventDefault(); input.focus(); input.select(); }
  });
})();
