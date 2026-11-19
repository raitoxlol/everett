# Everett docs site

A static documentation site generated from the repository's own Markdown. `README.md`,
`docs/`, `plugins/*/README.md`, and `CHANGELOG.md` stay the source of truth; this folder only
arranges them into pages and adds a few overview pages.

```bash
cd site
npm ci
npm run build        # writes site/dist
npm run check        # build and fail on unresolved links or anchors
npm run serve        # preview at http://127.0.0.1:4173/
```

Set `SITE_URL` (for example `https://raitoxlol.github.io/everett`) to use absolute URLs in the
Markdown exports, `llms.txt`, and `sitemap.xml`. The HTML uses relative links and works under any
base path. `.github/workflows/docs.yml` builds with `--strict` on pull requests and deploys
`main` to GitHub Pages (enable Pages with the "GitHub Actions" source).

## Layout

| Path | Purpose |
| --- | --- |
| `nav.mjs` | Groups, page order, slugs, and each page's source, `summary`, and `readWhen` |
| `pages/*.md` | Authored pages (`authored: true` in `nav.mjs`): introduction, concepts, agent guide |
| `build.mjs` | Assembles pages, rewrites links, renders HTML, writes agent exports |
| `assets/` | `style.css` (the raitox.tech/everett design language: ink, bone, sun, momo, Geist), `app.js`, Geist fonts (OFL), agent icon sprite and its notices |

A page sourced from `README.md` names one or more headings in `sections`; it takes each heading
and everything under it. `exclude` drops a sub-section. A page sourced from a whole file uses that
file's H1 position as the start. Links between source files (`docs/x.md#anchor`, `#readme-anchor`)
are rewritten to the page that now holds that heading, so keep writing ordinary GitHub-relative
links in the docs.

## Adding a page

1. Add a `docs/` file (or a `README.md` section), or a `pages/<slug>.md` file for site-only prose.
2. Add an entry to a group in `nav.mjs` with a one-sentence `summary` and a `readWhen` hint.
3. Run `npm run check`.

## Agent-facing output

| URL | Contents |
| --- | --- |
| `/<slug>/` | HTML page |
| `/<slug>.md` | The same page as Markdown with front matter (`title`, `summary`, `read_when`, `source`) |
| `/llms.txt` | Index of every page with summary and *read when* hint ([llmstxt.org](https://llmstxt.org)) |
| `/llms-full.txt` | Every page in one file |
| `/search.json` | Section-level search index used by the site search |

Every HTML page also advertises its Markdown twin with `<link rel="alternate" type="text/markdown">`.
