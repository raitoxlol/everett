# everett.raitox.tech

Static site for Everett. Plain HTML/CSS/vanilla JS — no framework, no build step.

## Deploy (Vercel)

1. Import the repo in Vercel → New Project → `raitoxlol/everett`.
2. **Root Directory:** `site` (click "Edit" next to Root Directory, select `site/`).
3. **Framework Preset:** Other. Leave Build/Output empty — the directory deploys as-is.
4. **DNS** (raitox.tech zone, e.g. Cloudflare): add
   `CNAME everett → cname.vercel-dns.com`, then attach `everett.raitox.tech`
   as the project domain in Vercel → Settings → Domains.

`vercel.json` in this directory sets clean URLs, no trailing slash, and the
security headers (CSP, nosniff, Referrer-Policy, frame deny).

## Files

- `index.html` — the whole site
- `styles.css` — light default, `prefers-color-scheme: dark` support
- `site.js` — copy buttons + reduced-motion-aware scroll reveal (optional)
- `404.html`, `favicon.svg`, `og.png` (1200×630, generated with PIL)
- `robots.txt`, `vercel.json`

## Local preview

```sh
python3 -m http.server 8080 -d site
```
