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
    if (!['GET', 'HEAD'].includes(req.method)) {
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
    res.writeHead(response.status, Object.fromEntries(response.headers));
    res.end(Buffer.from(await response.arrayBuffer()));
  } catch {
    reject(503);
  }
}
