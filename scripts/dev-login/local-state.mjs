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
  const caFile = path.join(state, 'ca.pem');
  const caKey = path.join(state, 'ca-key.pem');
  let valid = false;
  try {
    const certificate = new X509Certificate(fs.readFileSync(cert));
    const ca = new X509Certificate(fs.readFileSync(caFile));
    const now = Date.now();
    valid = Date.parse(certificate.validFrom) <= now && now < Date.parse(certificate.validTo)
      && Date.parse(ca.validFrom) <= now && now < Date.parse(ca.validTo)
      && ca.ca && !certificate.ca && certificate.verify(ca.publicKey)
      && ca.checkPrivateKey(createPrivateKey(fs.readFileSync(caKey)))
      && certificate.checkHost('localhost') === 'localhost'
      && certificate.checkIP('127.0.0.1') === '127.0.0.1'
      && certificate.checkPrivateKey(createPrivateKey(fs.readFileSync(key)));
  } catch {}
  if (!valid) {
    const config = path.join(state, 'openssl.cnf');
    fs.writeFileSync(config, '[req]\ndistinguished_name=dn\nx509_extensions=ca\nprompt=no\n[dn]\nCN=Everett local development CA\n[ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n[server]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n');
    const csr = path.join(state, 'tls.csr');
    for (const args of [
      ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '30', '-keyout', caKey, '-out', caFile, '-config', config],
      ['req', '-new', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', csr, '-subj', '/CN=localhost'],
      ['x509', '-req', '-in', csr, '-CA', caFile, '-CAkey', caKey, '-CAcreateserial', '-days', '30', '-out', cert, '-extfile', config, '-extensions', 'server'],
    ]) {
      if (spawnSync('openssl', args, { stdio: 'ignore' }).status !== 0) throw new Error('openssl failed to create the local CA and TLS certificate');
    }
    fs.rmSync(csr);
  }
  for (const file of [caKey, key]) fs.chmodSync(file, 0o600);
  return { key: fs.readFileSync(key), cert: fs.readFileSync(cert), ca: fs.readFileSync(caFile) };
}
