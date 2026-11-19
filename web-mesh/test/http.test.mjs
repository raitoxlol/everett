import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { parse } from 'node:querystring';
import test from 'node:test';
import login from '../api/login.mjs';

test('accepts a login after the platform consumes the form stream', async () => {
  const server = createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    req.body = parse(Buffer.concat(chunks).toString('utf8'));
    await login(req, res);
  }).listen(0, '127.0.0.1');
  try {
    await once(server, 'listening');
    const origin = 'http://127.0.0.1:' + server.address().port;
    const secret = 'local fixture + % & '.repeat(2);
    Object.assign(process.env, {
      ENABLED: 'true', MESH_MODE: 'fixture', CONSOLE_ORIGIN: origin,
      CONSOLE_SECRET: secret, SESSION_KEY: 'k'.repeat(64),
    });
    const request = body => fetch(origin + '/api/login', {
      method: 'POST',
      headers: { Origin: origin, 'Content-Type': 'application/x-www-form-urlencoded' },
      body,
    });
    const form = new URLSearchParams({ secret });
    const approved = await request(form);
    assert.equal(approved.status, 204);
    assert.match(approved.headers.get('Set-Cookie'), /HttpOnly/);
    assert.equal((await request(form + '&secret=extra')).status, 401);
    assert.equal((await request('secret=' + 'a'.repeat(5000))).status, 413);
  } finally {
    server.close();
    await once(server, 'close');
  }
});
