import { createConsole } from './server.mjs';

export default async function handler(req, res) {
  const reject = status => {
    res.writeHead(status, { 'Cache-Control': 'no-store' });
    res.end();
  };
  if (process.env.ENABLED !== 'true') return reject(404);

  try {
    const origin = new URL(process.env.CONSOLE_ORIGIN);
    if (req.headers.host?.toLowerCase() !== origin.host) return reject(403);
    const url = new URL(req.url, origin);
    if (url.origin !== origin.origin) return reject(403);

    const headers = new Headers();
    for (const [name, value] of Object.entries(req.headers)) {
      if (value !== undefined) headers.set(name, Array.isArray(value) ? value.join(',') : value);
    }
    let body;
    const contentType = headers.get('Content-Type')?.split(';', 1)[0].trim();
    const bufferedLogin = req.method === 'POST' && url.pathname === '/api/login' &&
      contentType === 'application/x-www-form-urlencoded' && 'body' in req;
    if (bufferedLogin) {
      if (Number(headers.get('Content-Length')) > 4096) return reject(413);
      const fields = req.body;
      if (fields && Object.keys(fields).length === 1 && typeof fields.secret === 'string') {
        body = new URLSearchParams({ secret: fields.secret }).toString();
        if (body.length > 4096) return reject(413);
      }
    } else if (!['GET', 'HEAD'].includes(req.method)) {
      const chunks = [];
      let size = 0;
      for await (const chunk of req) {
        size += chunk.length;
        if (size > 4096) return reject(413);
        chunks.push(chunk);
      }
      body = Buffer.concat(chunks);
    }

    const request = new Request(url, { method: req.method, headers, body });
    const response = await createConsole(process.env)(request);
    const responseHeaders = Object.fromEntries(response.headers);
    const cookies = response.headers.getSetCookie();
    if (cookies.length) responseHeaders['set-cookie'] = cookies;
    res.writeHead(response.status, responseHeaders);
    res.end(Buffer.from(await response.arrayBuffer()));
  } catch {
    reject(503);
  }
}
