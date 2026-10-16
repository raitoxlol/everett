import fs from 'node:fs';
import path from 'node:path';
import { X509Certificate, createPrivateKey } from 'node:crypto';
import { spawnSync } from 'node:child_process';

export function seedSession(home, session, cwd) {
  const timestamp = new Date().toISOString();
  const folder = path.join(home, '.codex', 'sessions', ...timestamp.slice(0, 10).split('-'));
  fs.mkdirSync(folder, { recursive: true });
  const rows = [
    { timestamp, type: 'session_meta', payload: { id: session, cwd, timestamp } },
    { timestamp, type: 'response_item', payload: { type: 'message', role: 'user', content: [{ type: 'input_text', text: 'Local relay smoke session fixture' }] } },
  ];
  fs.writeFileSync(path.join(folder, `${session}.jsonl`), rows.map(value => JSON.stringify(value)).join('\n') + '\n');
}

export function syncConvexFunctions(root, operator) {
  const destination = path.join(operator, 'convex');
  fs.rmSync(destination, { recursive: true, force: true });
  fs.cpSync(path.join(root, 'convex'), destination, { recursive: true, filter: source => !source.split(path.sep).includes('_generated') });
}

export function ensureTlsCertificate(state) {
  const key = path.join(state, 'tls-key.pem');
  const cert = path.join(state, 'tls-cert.pem');
  let valid = false;
  try {
    const certificate = new X509Certificate(fs.readFileSync(cert));
    const now = Date.now();
    valid = Date.parse(certificate.validFrom) <= now && now < Date.parse(certificate.validTo)
      && certificate.checkHost('localhost') === 'localhost'
      && certificate.checkIP('127.0.0.1') === '127.0.0.1'
      && certificate.checkPrivateKey(createPrivateKey(fs.readFileSync(key)));
  } catch {}
  if (!valid) {
    const config = path.join(state, 'openssl.cnf');
    fs.writeFileSync(config, '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,digitalSignature,keyEncipherment,keyCertSign\nextendedKeyUsage=serverAuth\n');
    const result = spawnSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '30', '-keyout', key, '-out', cert, '-config', config], { stdio: 'ignore' });
    if (result.status !== 0) throw new Error('openssl failed to create the local TLS certificate');
  }
  return { key: fs.readFileSync(key), cert: fs.readFileSync(cert) };
}
