#!/usr/bin/env node
// Builds the Everett docs site into site/dist.
//
// Sources: README.md, docs/*.md, plugins/*/README.md, CHANGELOG.md (assembled per
// site/nav.mjs) and site/pages/*.md (authored overview pages). Output per page:
//   <slug>/index.html   the human page
//   <slug>.md           the same content as Markdown with front matter, for agents
// plus llms.txt, llms-full.txt, search.json, sitemap.xml (when SITE_URL is set), 404.html.
//
//   node build.mjs            build
//   node build.mjs --strict   build and fail on unresolved links or anchors
//   SITE_URL=https://example.com/everett node build.mjs   absolute URLs in Markdown exports

import fs from 'node:fs';
import path from 'node:path';
import { execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { Marked, Renderer } from 'marked';
import { site, groups } from './nav.mjs';

const SITE_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(SITE_DIR, '..');
const OUT = path.join(SITE_DIR, 'dist');
const SITE_URL = (process.env.SITE_URL || '').replace(/\/+$/, '');
const STRICT = process.argv.includes('--strict');
const problems = [];

const version = (fs.readFileSync(path.join(ROOT, 'everett-rs/Cargo.toml'), 'utf8').match(/^version\s*=\s*"([^"]+)"/m) || [])[1] || '';
let commit = '';
try { commit = execSync('git rev-parse --short HEAD', { cwd: ROOT, stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim(); } catch {}


const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const read = (rel) => fs.readFileSync(path.join(ROOT, rel), 'utf8');
const write = (rel, data) => { const p = path.join(OUT, rel); fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, data); };

// Heading text as GitHub sees it for anchors.
const plain = (md) => md
  .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
  .replace(/`([^`]*)`/g, '$1')
  .replace(/\*\*|__/g, '')
  .replace(/(^|\W)\*(\S[^*]*)\*/g, '$1$2')
  .replace(/<[^>]+>/g, '')
  .trim();

function makeSlugger() {
  const seen = new Map();
  return (text) => {
    const base = plain(text).toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, '').replace(/\s/g, '-');
    const n = seen.get(base);
    seen.set(base, n === undefined ? 1 : n + 1);
    return n === undefined ? base : `${base}-${n}`;
  };
}

const HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
const FENCE = /^(?: {0,3})(`{3,}|~{3,})(.*)$/;

function* fencedLines(lines) {
  let fence = null;
  for (const line of lines) {
    const match = line.match(FENCE);
    const opens = match && (match[1][0] !== '`' || !match[2].includes('`'));
    const fenced = Boolean(fence || opens);
    if (!fence && opens) fence = match[1];
    else if (fence && match && match[1].startsWith(fence) && !match[2].trim()) fence = null;
    yield { line, fenced };
  }
}

function scanHeadings(lines) {
  const out = [];
  let i = 0;
  for (const { line, fenced } of fencedLines(lines)) {
    const heading = !fenced && line.match(HEADING);
    if (heading) out.push({ i, level: heading[1].length, text: heading[2] });
    i++;
  }
  return out;
}

const sourceCache = new Map();
function source(file) {
  if (!sourceCache.has(file)) {
    const lines = read(file).replace(/\r\n/g, '\n').split('\n');
    const slug = makeSlugger();
    const headings = scanHeadings(lines).map((h) => ({ ...h, slug: slug(h.text) }));
    sourceCache.set(file, { lines, headings });
  }
  return sourceCache.get(file);
}

function sectionRange(src, name, file) {
  const idx = src.headings.findIndex((h) => plain(h.text) === name);
  if (idx < 0) throw new Error(`Section "${name}" not found in ${file}`);
  const h = src.headings[idx];
  const next = src.headings.slice(idx + 1).find((n) => n.level <= h.level);
  return { start: h.i, end: next ? next.i : src.lines.length, level: h.level };
}

// A page body is a list of lines; heading lines carry the source anchor they came from.
function assemble(page) {
  if (page.authored) {
    const text = fs.readFileSync(path.join(SITE_DIR, 'pages', `${page.slug}.md`), 'utf8');
    return { lines: text.split('\n').map((t) => ({ t })), file: null, topAnchors: [] };
  }
  const src = source(page.file);
  const meta = new Map(src.headings.map((h) => [h.i, h]));
  const topAnchors = [];
  let ranges;
  if (page.sections) {
    ranges = page.sections.map((s) => sectionRange(src, s, page.file));
  } else {
    const h1 = src.headings.find((h) => h.level === 1);
    if (h1) topAnchors.push(h1.slug);
    ranges = [{ start: h1 ? h1.i + 1 : 0, end: src.lines.length, level: 1, noHeading: true }];
  }
  const excluded = new Set();
  for (const name of page.exclude || []) {
    const r = sectionRange(src, name, page.file);
    for (let i = r.start; i < r.end; i++) excluded.add(i);
  }
  const single = ranges.length === 1;
  const lines = [];
  for (const r of ranges) {
    // Shift so the section's children (single section) or the section itself (several) become H2.
    const shift = r.noHeading
      ? Math.min(...src.headings.filter((h) => h.i >= r.start && h.i < r.end).map((h) => h.level), 99) - 2
      : single ? r.level - 1 : r.level - 2;
    for (let i = r.start; i < r.end; i++) {
      if (excluded.has(i)) continue;
      const h = meta.get(i);
      if (h && !r.noHeading && i === r.start && single) { topAnchors.push(h.slug); continue; }
      if (h) {
        const level = Math.min(6, Math.max(2, h.level - (shift > 0 ? shift : 0)));
        lines.push({ t: `${'#'.repeat(level)} ${h.text}`, src: h.slug });
      } else {
        lines.push({ t: src.lines[i] });
      }
    }
    lines.push({ t: '' });
  }
  return { lines, file: page.file, topAnchors };
}


const pages = [];
groups.forEach((g) => g.pages.forEach((p, i) => pages.push({ ...p, group: g, pos: i + 1 })));
pages.forEach((p, n) => { p.index = n; });
const bySlug = new Map(pages.map((p) => [p.slug, p]));
if (bySlug.size !== pages.length) throw new Error('Duplicate page slug in nav.mjs');

const fileToPage = new Map();   // repo file -> slug (whole-file pages, else first user)
const anchorMap = new Map();    // "file#anchor" -> {slug, anchor}

for (const p of pages) {
  const body = assemble(p);
  p.body = body;
  if (p.file) {
    for (const a of body.topAnchors) anchorMap.set(`${p.file}#${a}`, { slug: p.slug, anchor: '' });
  }
  // Page-local anchors, computed the way the renderer will compute them.
  const slug = makeSlugger();
  p.anchors = new Set();
  for (const h of scanHeadings(body.lines.map((l) => l.t))) {
    const a = slug(h.text);
    p.anchors.add(a);
    const line = body.lines[h.i];
    if (p.file && line.src) anchorMap.set(`${p.file}#${line.src}`, { slug: p.slug, anchor: a });
  }
}
for (const p of pages) if (p.file && !p.sections) fileToPage.set(p.file, p.slug);
if (!fileToPage.has('README.md')) fileToPage.set('README.md', 'introduction');


const REPO_BLOB = `${site.repo}/blob/${site.branch}/`;
const MEDIA_PREFIX = 'docs/assets/';

// Returns {page:{slug,anchor}} | {asset:path} | {href:url}
function resolveLink(url, file, page) {
  if (/^(mailto:|tel:)/i.test(url)) return { href: url };
  if (url.startsWith(REPO_BLOB)) {
    const rest = url.slice(REPO_BLOB.length);
    const [p, a = ''] = rest.split('#');
    if (fileToPage.has(p)) return resolveRepoPath(p, a, url, page);
    return { href: url };
  }
  if (/^[a-z][a-z0-9+.-]*:/i.test(url) || url.startsWith('//')) return { href: url };
  if (url.startsWith('#')) {
    const a = url.slice(1);
    if (!file) return checkAnchor(page.slug, a, url, page);
    return resolveRepoPath(file, a, url, page);
  }
  if (url.startsWith('/')) {
    const [p, a = ''] = url.slice(1).split('#');
    const slug = p.replace(/\/+$/, '') || 'index';
    if (slug === 'index' || bySlug.has(slug)) return checkAnchor(slug, a, url, page);
    return { asset: p + (a ? `#${a}` : '') };
  }
  const [p, a = ''] = url.split('#');
  const repoPath = path.posix.normalize(path.posix.join(file ? path.posix.dirname(file) : 'site/pages', decodeURI(p)));
  return resolveRepoPath(repoPath, a, url, page);
}

function checkAnchor(slug, anchor, url, page) {
  if (anchor && slug !== 'index' && !bySlug.get(slug).anchors.has(anchor)) {
    problems.push(`${page.slug}: anchor not found: ${url}`);
  }
  return { page: { slug, anchor } };
}

function resolveRepoPath(repoPath, anchor, url, page) {
  if (anchor && anchorMap.has(`${repoPath}#${anchor}`)) return { page: anchorMap.get(`${repoPath}#${anchor}`) };
  if (fileToPage.has(repoPath)) {
    const slug = fileToPage.get(repoPath);
    if (anchor) {
      if (bySlug.get(slug).anchors.has(anchor)) return { page: { slug, anchor } };
      problems.push(`${page.slug}: anchor not found: ${url} (in ${repoPath})`);
    }
    return { page: { slug, anchor: '' } };
  }
  if (repoPath.startsWith(MEDIA_PREFIX)) return { asset: `media/${repoPath.slice(MEDIA_PREFIX.length)}` };
  if (!fs.existsSync(path.join(ROOT, repoPath))) problems.push(`${page.slug}: missing repository file: ${url} -> ${repoPath}`);
  return { href: `${REPO_BLOB}${repoPath}${anchor ? `#${anchor}` : ''}` };
}

// Rewrites Markdown links outside fenced code. Link labels may span lines or contain
// inline code. `fmt` turns a resolution into a URL string.
const LINK = /(!?\[(?:[^\[\]]|\[[^\]]*\])*\])\(\s*<?([^)\s>]+)>?(\s+"[^"]*")?\s*\)/g;
function rewriteLinks(text, file, page, fmt) {
  const out = [];
  let prose = [];
  const flush = () => {
    if (!prose.length) return;
    out.push(prose.join('\n').replace(LINK, (_, label, url, title = '') => `${label}(${fmt(resolveLink(url, file, page))}${title})`));
    prose = [];
  };
  for (const { line, fenced } of fencedLines(text.split('\n'))) {
    if (fenced) {
      flush();
      out.push(line);
    } else prose.push(line);
  }
  flush();
  return out.join('\n');
}

const pageUrl = (slug) => (slug === 'index' ? '' : `${slug}/`);
const htmlFmt = (root) => (r) => {
  if (r.page) return `${root}${pageUrl(r.page.slug)}${r.page.anchor ? `#${r.page.anchor}` : ''}` || './';
  if (r.asset) return `${root}${r.asset}`;
  return r.href;
};
const mdFmt = (r) => {
  if (r.page) return `${SITE_URL}/${r.page.slug === 'index' ? 'index' : r.page.slug}.md${r.page.anchor ? `#${r.page.anchor}` : ''}`;
  if (r.asset) return `${SITE_URL}/${r.asset}`;
  return r.href;
};



const HL = {
  comment: (s) => `<span class="tok-c">${s}</span>`,
};
function highlight(code, lang) {
  const e = esc(code);
  const l = (lang || '').toLowerCase();
  if (['bash', 'sh', 'shell', 'zsh', 'console', 'toml', 'yaml', 'yml'].includes(l)) {
    return e.split('\n').map((line) => {
      const m = line.match(/^(.*?)(^|\s)(#(?!!).*)$/);
      let out = m && !/["'][^"']*$/.test(m[1]) ? `${m[1]}${m[2]}${HL.comment(m[3])}` : line;
      if (l === 'toml') out = out.replace(/^(\s*)([A-Za-z0-9_.-]+)(\s*=)/, '$1<span class="tok-k">$2</span>$3').replace(/^(\s*)(\[[^\]]+\])/, '$1<span class="tok-k">$2</span>');
      if (['bash', 'sh', 'shell', 'zsh', 'console'].includes(l)) out = out.replace(/^(\$ )/, '<span class="tok-p">$1</span>');
      return out;
    }).join('\n');
  }
  if (l === 'json' || l === 'jsonc') {
    return e.replace(/(&quot;(?:[^&]|&(?!quot;))*?&quot;)(\s*:)/g, '<span class="tok-k">$1</span>$2')
      .replace(/(:\s*)(&quot;(?:[^&]|&(?!quot;))*?&quot;)/g, '$1<span class="tok-s">$2</span>');
  }
  if (l === 'text' && /^\$ /m.test(code)) return e.replace(/^(\$ )(.*)$/gm, '<span class="tok-p">$1</span><span class="tok-cmd">$2</span>');
  return e;
}

const defaultTable = Renderer.prototype.table;

const inlineRenderer = {
  html({ text }) { return esc(text); },
  link({ href, title, tokens }) {
    const inner = this.parser.parseInline(tokens);
    const ext = /^https?:\/\//.test(href);
    return `<a href="${esc(href)}"${title ? ` title="${esc(title)}"` : ''}${ext ? ' rel="noopener" class="ext"' : ''}>${inner}</a>`;
  },
};

function renderMarkdown(md) {
  const slugger = makeSlugger();
  const headings = new Map();
  let h2Count = 0;
  const marked = new Marked({ gfm: true });
  marked.use({
    renderer: {
      heading(token) {
        const { tokens, depth, text } = token;
        const id = slugger(text);
        const inner = this.parser.parseInline(tokens);
        if (depth === 2 || depth === 3) headings.set(token, { depth, id, text: plain(text) });
        const n = depth === 2 ? `<span class="n" aria-hidden="true">${String(++h2Count).padStart(2, '0')}</span>` : '';
        const body = inner.includes('<a ') ? inner : `<a class="hl" href="#${esc(id)}">${inner}</a>`;
        return `<h${depth} id="${esc(id)}">${n}${body}</h${depth}>\n`;
      },
      code({ text, lang }) {
        const language = (lang || '').split(/\s/)[0];
        const label = language ? `<span class="code-lang">${esc(language)}</span>` : '';
        return `<div class="code">${label}<button class="cp" type="button" data-copy aria-label="Copy code">copy</button><pre><code>${highlight(text, language)}</code></pre></div>\n`;
      },
      table(token) {
        return `<div class="table-wrap">${defaultTable.call(this, token)}</div>\n`;
      },
      ...inlineRenderer,
      blockquote({ tokens }) {
        const body = this.parser.parse(tokens);
        return `<blockquote>${body}</blockquote>\n`;
      },
    },
  });
  const tokens = marked.lexer(md);
  const html = marked.parser(tokens);
  const sections = [{ id: '', head: '', text: [] }];
  for (const token of tokens) {
    const heading = headings.get(token);
    if (heading) {
      sections.push({ id: heading.id, head: heading.text, text: [] });
    } else if (token.type !== 'code' || token.codeBlockStyle === 'indented') {
      sections.at(-1).text.push(token.raw);
    }
  }
  return { html, toc: [...headings.values()], sections };
}

const inlineMarkdown = new Marked({ gfm: true });
inlineMarkdown.use({ renderer: inlineRenderer });

// Design language: raitox.tech/everett (project.css, everett-setup.css).

const readingMinutes = (md) => Math.max(1, Math.round(md.split(/\s+/).filter(Boolean).length / 220));
const groupNo = (g) => String(groups.indexOf(g) + 1).padStart(2, '0');

// Canonical Everett M6 Many-Worlds Horizon Balloon mark (brand/assets/everett-mark.svg).
const PI = (cls = 'pi') => `<svg class="${cls}" viewBox="0 0 128 128" fill="none" aria-hidden="true"><path class="o" d="M23.22 54.00H104.78Q106.58 54.00 106.29 52.20A43.00 43.00 0 0 0 21.71 52.20Q21.42 54.00 23.22 54.00Z"/><path fill="currentColor" d="M13.50 62.00H114.50A3.50 3.50 0 0 1 114.50 69.00H35.51A4.00 4.00 0 0 0 35.51 77.00H100.90Q103.50 77.00 102.57 79.00A43.00 43.00 0 0 1 85.50 97.24L66.75 108.06A5.50 5.50 0 0 1 61.25 108.06L42.50 97.24A43.00 43.00 0 0 1 37.68 94.00Q35.28 92.00 37.88 92.00H84.13A4.00 4.00 0 0 0 84.13 84.00H30.92Q28.32 84.00 27.05 82.00A43.00 43.00 0 0 1 22.95 72.80Q21.95 69.00 18.15 69.00H13.50A3.50 3.50 0 0 1 13.50 62.00Z"/></svg>`;

function navHtml(root, current) {
  return groups.map((g) => `
      <div class="side-group">
        <p class="side-title"><span class="n">${groupNo(g)}</span>${esc(g.title)}</p>
        <ul>${g.pages.map((p) => `
          <li><a href="${root}${pageUrl(p.slug)}"${p.slug === current ? ' aria-current="page"' : ''}>${esc(p.title)}</a></li>`).join('')}
        </ul>
      </div>`).join('');
}

function footHtml(root) {
  return `<footer class="foot">
          <span>Everett · MIT · v${esc(version)}${commit ? ` · ${esc(commit)}` : ''}</span>
          <nav aria-label="More"><a href="${site.repo}" rel="noopener">GitHub</a><a href="https://www.raitox.tech/everett" rel="noopener">raitox.tech/everett</a><a href="${root}llms.txt">llms.txt</a></nav>
        </footer>`;
}

function layout({ root, title, description, current, body, mdHref, bodyClass = '', side = true }) {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)}</title>
<meta name="description" content="${esc(description)}">
<meta name="theme-color" content="#0E0C0B">
<meta name="color-scheme" content="dark">
<meta property="og:title" content="${esc(title)}">
<meta property="og:description" content="${esc(description)}">
<meta property="og:image" content="${SITE_URL ? `${SITE_URL}/` : root}assets/og-card.png">
${mdHref ? `<link rel="alternate" type="text/markdown" href="${esc(mdHref)}" title="Markdown">\n` : ''}<link rel="alternate" type="text/plain" href="${root}llms.txt" title="llms.txt">
<link rel="icon" href="${root}assets/favicon.svg" type="image/svg+xml">
<link rel="preload" href="${root}assets/fonts/Sora-var.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="${root}assets/style.css">
</head>
<body class="${bodyClass}" data-root="${root}">
<a class="skip" href="#main">Skip to content</a>
<header class="top">
  <a class="brand" href="${root}">${PI('pi pi-sm')}<span>Everett</span><span class="sep">/</span><span class="dim">docs</span></a>
  <div class="search" role="search">
    <label class="search-field"><span class="sr-only">Search docs</span>
      <input id="search" type="search" placeholder="search docs, commands, tools" autocomplete="off" spellcheck="false"
        role="combobox" aria-expanded="false" aria-controls="search-results" aria-autocomplete="list">
      <kbd>/</kbd>
    </label>
    <div class="search-results" id="search-results" role="listbox" hidden></div>
  </div>
  <nav class="top-links" aria-label="Site">
    <a href="${root}agent-guide/">for agents</a>
    <a href="${root}llms.txt">llms.txt</a>
  </nav>
  <a class="gh-btn" href="${site.repo}" rel="noopener"><svg class="gh-mark" viewBox="0 0 16 16" aria-hidden="true"><path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z"/></svg><span>GitHub</span><span class="arr" aria-hidden="true">↗</span></a>
  ${side ? '<button class="menu" type="button" aria-controls="side" aria-expanded="false" data-nav-toggle>menu</button>' : ''}
</header>
${body}
<script src="${root}assets/app.js" defer></script>
</body>
</html>
`;
}

function docPage(p) {
  const root = '../';
  const htmlMd = rewriteLinks(p.body.lines.map((l) => l.t).join('\n'), p.body.file, p, htmlFmt(root));
  const { html, toc: pageToc, sections } = renderMarkdown(htmlMd);
  const prev = pages[p.index - 1];
  const next = pages[p.index + 1];
  const sourceLabel = p.file ? `${p.file}${p.sections ? ` · ${p.sections.join(', ')}` : ''}` : `site/pages/${p.slug}.md`;
  const sourceUrl = `${REPO_BLOB}${p.file || `site/pages/${p.slug}.md`}`;
  const docNo = `${groupNo(p.group)}.${String(p.pos).padStart(2, '0')}`;
  const tocHtml = pageToc.length > 1 ? `
    <aside class="toc" aria-label="On this page">
      <p class="toc-title">on this page</p>
      <ol>${pageToc.map((t) => `<li class="d${t.depth}"><a href="#${esc(t.id)}">${esc(t.text)}</a></li>`).join('')}</ol>
    </aside>` : '<aside class="toc" aria-hidden="true"></aside>';
  const body = `<div class="shell">
  <nav class="side" id="side" aria-label="Documentation">${navHtml(root, p.slug)}
  </nav>
  <main id="main" class="doc" tabindex="-1">
    <header class="doc-head">
      <p class="mark"><span class="n">${docNo}</span>${esc(p.group.title)}<span class="dot" aria-hidden="true">·</span>${readingMinutes(htmlMd)} min</p>
      <h1>${esc(p.title)}</h1>
      <p class="intro">${renderInline(p.summary)}</p>
      <p class="meta"><span>read when</span> ${renderInline(p.readWhen)}</p>
      <p class="actions">
        <button type="button" class="cp" data-copy-page="${root}${p.slug}.md">copy page</button>
        <a class="cp" href="${root}${p.slug}.md">view .md</a>
        <a class="cp" href="${esc(sourceUrl)}" rel="noopener">source ↗</a>
      </p>
    </header>
    <article class="prose">
${html}
    </article>
    <nav class="pager" aria-label="Previous and next">
      ${prev ? `<a class="prev" href="${root}${pageUrl(prev.slug)}"><small>← previous</small><span>${esc(prev.title)}</span></a>` : '<span></span>'}
      ${next ? `<a class="next" href="${root}${pageUrl(next.slug)}"><small>next →</small><span>${esc(next.title)}</span></a>` : '<span></span>'}
    </nav>
    <p class="source">generated from <a href="${esc(sourceUrl)}" rel="noopener">${esc(sourceLabel)}</a></p>
    ${footHtml(root)}
  </main>${tocHtml}
</div>
<div class="scrim" data-close-nav hidden></div>`;
  return { html: layout({ root, title: `${p.title} · Everett docs`, description: p.summary, current: p.slug, body, mdHref: `${root}${p.slug}.md` }), sections };
}

function renderInline(md) {
  return inlineMarkdown.parseInline(md);
}

function homePage() {
  const root = './';
  const card = (href, icon, title, sub, ext = false) => `<a class="agent-card" href="${href}"${ext ? ' rel="noopener"' : ''}><span class="agent-icon">${icon}</span><span><strong>${esc(title)}</strong><small>${renderInline(sub)}</small></span><span class="arrow" aria-hidden="true">${ext ? '↗' : '→'}</span></a>`;
  const use = (id) => `<svg aria-hidden="true"><use href="${root}assets/icons.svg#${id}"/></svg>`;
  const glyph = (t) => `<span class="glyph" aria-hidden="true">${t}</span>`;
  const setup = 'https://www.raitox.tech/everett/setup/';
  const cmd = (c) => `<div class="cmd"><span>${esc(c)}</span><button class="cp" type="button" data-copy-text="${esc(c)}">copy</button></div>`;
  const body = `<main id="main" class="home" tabindex="-1">
  <header class="col hero">
    ${PI()}
    <p class="tag">documentation · v${esc(version)}</p>
    <h1>Everett docs</h1>
    <p class="say">Many sessions. One mind.</p>
    <p>Everything needed to install Everett, run it from the terminal, and let your agents use it through MCP: routing, messaging, events, shared memory, devices and integrations. Built from the <a class="u" href="${site.repo}" rel="noopener">repository</a>'s own docs.</p>
    <div class="setup-actions"><a class="setup-primary" href="${root}quickstart/">Start with the quickstart <span aria-hidden="true">→</span></a><a class="u" href="${site.repo}" rel="noopener">View on GitHub ↗</a></div>
    <p class="setup-note">Agents: read <a class="u" href="${root}llms.txt">/llms.txt</a>. Every page has a Markdown twin at <code>/&lt;page&gt;.md</code>; <a class="u" href="${root}llms-full.txt">/llms-full.txt</a> is the whole site in one file.</p>
  </header>

  <section class="col">
    <h2><span class="n" aria-hidden="true">Start here</span>Who is reading?</h2>
    <div class="agent-grid">
      ${card(`${root}quickstart/`, glyph('$_'), 'A person setting up', 'Install, onboard, then `everett doctor`')}
      ${card(`${root}agent-guide/`, PI('pi-mini'), 'An agent with Everett tools', 'Identity, routing-first sends, replies, memory')}
      ${card(`${root}concepts/`, glyph('[]'), 'Learning the model', 'Sessions, cards, routing, inboxes, core')}
      ${card(`${root}cli/`, glyph('--'), 'Looking something up', 'Every command, flag and exit code')}
    </div>
  </section>

  <section class="col">
    <h2><span class="n" aria-hidden="true">01</span>Install</h2>
    <ol class="cmds">
      <li><span class="k">Homebrew</span>${cmd('brew install raitoxlol/tap/everett')}</li>
      <li><span class="k">Or the native binary</span><p>macOS and Linux. Verifies the release checksum.</p>${cmd('curl -fsSL https://raw.githubusercontent.com/raitoxlol/everett/main/install.sh | sh')}</li>
      <li><span class="k">Then</span><p>A guided terminal setup, then a health check.</p>${cmd('everett onboard')}${cmd('everett doctor')}</li>
    </ol>
    <p class="setup-note">Scripting it? <a class="u" href="${root}onboarding/">Onboarding flags</a> cover <code>--yes</code>, backfill and MCP options.</p>
  </section>

  <section class="col">
    <h2><span class="n" aria-hidden="true">02</span>Connect your agent</h2>
    <p>Each agent has a setup guide with compatibility limits and a real connection check. The docs below explain what happens underneath.</p>
    <h3 class="setup-label">Terminal agents <span aria-hidden="true">/</span> macOS &amp; Linux</h3>
    <div class="agent-grid">
      ${card(`${setup}claude-code`, use('claudecode'), 'Claude Code', 'Hooks + MCP', true)}
      ${card(`${setup}codex`, use('codex'), 'Codex', 'Hooks + MCP', true)}
      ${card(`${setup}omp`, use('omp'), 'OMP', 'Extension + MCP', true)}
      ${card(`${setup}pi`, use('pi'), 'Pi', 'Extension tool bridge · no native MCP', true)}
      ${card(`${setup}hermes`, use('hermes'), 'Hermes Agent', 'Native plugin · MCP-only fallback', true)}
      ${card(`${setup}grok-cli`, use('grok'), 'Grok CLI', 'Local CLI · not Grok Bot', true)}
      ${card(`${setup}devin-cli`, use('devin'), 'Devin CLI', 'Session hooks + MCP', true)}
      ${card(`${root}t3code/`, glyph('T3'), 'T3 Code', 'Overlay on its provider sessions')}
    </div>
    <h3 class="setup-label">Hosted agents &amp; bots</h3>
    <div class="agent-grid">
      ${card(`${root}chatgpt/`, use('openai'), 'ChatGPT & Dots', 'Gateway · HTTPS MCP URL')}
      ${card(`${root}grok-bot/`, use('grok'), 'Grok Bot', 'External inbox · webhook wake')}
    </div>
  </section>

  ${groups.map((g, i) => `
  <section class="col">
    <h2><span class="n" aria-hidden="true">${String(i + 3).padStart(2, '0')}</span>${esc(g.title)}</h2>
    <ul class="index">${g.pages.map((p) => `
      <li><a href="${root}${pageUrl(p.slug)}"><strong>${esc(p.title)}</strong><span>${renderInline(p.summary)}</span></a></li>`).join('')}
    </ul>
  </section>`).join('')}

  <section class="col">
    ${footHtml(root)}
  </section>
</main>`;
  return layout({ root, title: 'Everett docs · many sessions, one mind', description: site.tagline, current: 'index', body, bodyClass: 'is-home', mdHref: `${root}index.md`, side: false });
}


fs.rmSync(OUT, { recursive: true, force: true });
fs.mkdirSync(OUT, { recursive: true });
fs.cpSync(path.join(SITE_DIR, 'assets'), path.join(OUT, 'assets'), { recursive: true });
if (fs.existsSync(path.join(ROOT, MEDIA_PREFIX))) fs.cpSync(path.join(ROOT, MEDIA_PREFIX), path.join(OUT, 'media'), { recursive: true });

const search = [];
const llmsFull = [];
const frontMatter = (o) => `---\n${Object.entries(o).filter(([, v]) => v).map(([k, v]) => `${k}: ${JSON.stringify(v)}`).join('\n')}\n---\n\n`;
const absUrl = (slug) => `${SITE_URL}/${pageUrl(slug)}`;

for (const p of pages) {
  const { html, sections } = docPage(p);
  write(`${p.slug}/index.html`, html);

  const md = rewriteLinks(p.body.lines.map((l) => l.t).join('\n'), p.body.file, p, mdFmt).replace(/\n{3,}/g, '\n\n').trim();
  const fm = frontMatter({
    title: p.title, summary: p.summary, read_when: p.readWhen, section: p.group.title,
    url: absUrl(p.slug), source: `${p.file || `site/pages/${p.slug}.md`}${p.sections ? `#${p.sections.join(', ')}` : ''}`,
  });
  write(`${p.slug}.md`, `${fm}# ${p.title}\n\n> ${p.summary}\n\n${md}\n`);
  llmsFull.push(`# ${p.title}\n\nURL: ${absUrl(p.slug)}\nSummary: ${p.summary}\nRead when: ${p.readWhen}\n\n${md.replace(/^(#{2,5}) /gm, '#$1 ')}\n`);

  for (const s of sections) {
    const text = plain(s.text.join('').replace(/```[\s\S]*?```/g, ' ').replace(/[|#>*-]+/g, ' ')).replace(/\s+/g, ' ').trim();
    search.push({ t: p.title, h: s.head, u: `${pageUrl(p.slug)}${s.id ? `#${s.id}` : ''}`, g: p.group.title, x: (s.head ? text : `${p.summary} ${text}`).slice(0, 600) });
  }
}

write('index.html', homePage());
write('search.json', JSON.stringify(search));

const indexMd = `${frontMatter({ title: 'Everett docs', summary: site.tagline, url: absUrl('index') })}# Everett docs\n\n> ${site.tagline}\n\n${groups.map((g) => `## ${g.title}\n\n${g.pages.map((p) => `- [${p.title}](${SITE_URL}/${p.slug}.md): ${p.summary}`).join('\n')}`).join('\n\n')}\n`;
write('index.md', indexMd);

const llms = `# Everett

> ${site.tagline}

Everett is one native binary for macOS and Linux that discovers the sessions your coding-agent harnesses (Claude Code, Codex, OMP, Pi, Hermes Agent, Grok CLI, Devin CLI, Muse Code; T3 Code as an overlay) already recorded, summarizes them with cards, routes requests between them, delivers messages and replies, records status events, and maintains a shared memory core. Agents use it through the \`everett mcp\` stdio server.

How to read these docs as an agent:
- Each entry below links to a Markdown version of the page. Every HTML page at /<slug>/ has a Markdown twin at /<slug>.md.
- If you have Everett MCP tools, read "Agent operating guide" first, then "MCP tools reference".
- /llms-full.txt contains every page in one file.
- Source of truth: ${site.repo} (README.md and docs/). Version ${version}${commit ? `, commit ${commit}` : ''}.

${groups.map((g) => `## ${g.title}\n\n${g.pages.map((p) => `- [${p.title}](${SITE_URL}/${p.slug}.md): ${p.summary} Read when: ${p.readWhen.replace(/`/g, '')}`).join('\n')}`).join('\n\n')}
`;
write('llms.txt', llms);
write('llms-full.txt', `# Everett documentation (full)\n\n> ${site.tagline}\n\nVersion ${version}${commit ? `, commit ${commit}` : ''}. Source: ${site.repo}\n\n${llmsFull.join('\n---\n\n')}`);

write('404.html', layout({
  root: `${SITE_URL}/`, title: 'Not found · Everett docs', description: 'Page not found', current: '', side: false, bodyClass: 'is-home',
  body: `<main id="main" class="home" tabindex="-1"><header class="col hero">${PI()}<p class="tag">404 · undeliverable</p><h1>No page at this address</h1><p>It may have moved. Search above, or start from the <a class="u" href="${SITE_URL}/">docs home</a> or <a class="u" href="${SITE_URL}/llms.txt">llms.txt</a>.</p></header></main>`,
}));
write('.nojekyll', '');
if (SITE_URL) {
  write('sitemap.xml', `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${['index', ...pages.map((p) => p.slug)].map((s) => `  <url><loc>${esc(absUrl(s))}</loc></url>`).join('\n')}\n</urlset>\n`);
  write('robots.txt', `User-agent: *\nAllow: /\nSitemap: ${SITE_URL}/sitemap.xml\n`);
}


const unique = [...new Set(problems)];
console.log(`Built ${pages.length + 1} pages into ${path.relative(process.cwd(), OUT) || OUT} (v${version}${commit ? ` @ ${commit}` : ''}).`);
if (unique.length) {
  console.log(`\n${unique.length} link problem(s):`);
  for (const p of unique) console.log(`  - ${p}`);
  if (STRICT) process.exit(1);
}
