const $ = id => document.getElementById(id);
const make = (tag, text, className) => {
  const el = document.createElement(tag);
  if (text !== undefined) el.textContent = text;
  if (className) el.className = className;
  return el;
};

const views = {
  overview: ['Overview', 'Mesh overview', 'Devices, cloud connections, and delivery activity.'],
  devices: ['Devices', 'Devices', 'Find registered devices and sessions exposed by them.'],
  cloud: ['Cloud agents', 'Cloud agents', 'Hosted identities are tracked separately from device sessions.'],
  activity: ['Activity', 'Activity', 'Follow signed delivery observations across the mesh.'],
  settings: ['Settings', 'Settings', 'Review this console’s access and customize your workspace.'],
};
const defaultPrefs = { theme: 'dark', density: 'comfortable', 'time-format': 'relative' };
const choices = { theme: ['dark', 'light', 'system'], density: ['comfortable', 'compact'], 'time-format': ['relative', 'absolute'] };
const systemTheme = matchMedia('(prefers-color-scheme: dark)');
const prefKey = 'everett.mesh.display.v1';
let prefs = { ...defaultPrefs };
let snapshot = null;
let pending = null;
let approved = false;
let currentView = 'overview';

const viewResults = {
  overview: ['metrics', 'device-summary', 'recent-receipts'],
  devices: ['device-list', 'session-list'],
  cloud: ['cloud-list'],
  activity: ['receipt-list'],
  settings: ['connection'],
};

try {
  const saved = JSON.parse(localStorage.getItem(prefKey) || '{}');
  for (const key of Object.keys(prefs)) {
    if (choices[key].includes(saved?.[key])) prefs[key] = saved[key];
  }
} catch {}

function applyPrefs(save = false) {
  for (const [key, value] of Object.entries(prefs)) $(key).value = value;
  document.documentElement.dataset.theme =
    prefs.theme === 'system' ? (systemTheme.matches ? 'dark' : 'light') : prefs.theme;
  document.documentElement.dataset.density = prefs.density;
  if (save) {
    try { localStorage.setItem(prefKey, JSON.stringify(prefs)); } catch {}
  }
  updateTimes();
}

function setView() {
  const hash = location.hash.slice(1);
  const selected = Object.hasOwn(views, hash) ? hash : 'overview';
  if (selected !== currentView) {
    for (const id of viewResults[currentView]) $(id).replaceChildren();
    currentView = selected;
  }
  const [label, title, description] = views[selected];
  $('title').textContent = title;
  $('breadcrumb').textContent = label;
  $('description').textContent = description;
  for (const panel of document.querySelectorAll('[data-view]')) {
    panel.hidden = panel.dataset.view !== selected;
  }
  for (const link of document.querySelectorAll('.nav a')) {
    if (link.hash === '#' + selected) link.setAttribute('aria-current', 'page');
    else link.removeAttribute('aria-current');
  }
  if (snapshot) drawView();
}

function notice(text, kind = '') {
  const el = $('notice');
  if (el.textContent !== text) el.textContent = text;
  el.className = 'notice ' + kind;
}

function source(label, kind) {
  const el = $('source');
  el.textContent = label;
  el.dataset.source = kind;
}

function timestamp(at) {
  if (prefs['time-format'] === 'absolute') return new Date(at).toLocaleString();
  const delta = Math.round((Date.now() - at) / 1000);
  if (delta < 0) return 'Clock ahead';
  if (delta < 60) return delta + 's ago';
  if (delta < 3600) return Math.floor(delta / 60) + 'm ago';
  if (delta < 86400) return Math.floor(delta / 3600) + 'h ago';
  return Math.floor(delta / 86400) + 'd ago';
}

function clock(at) {
  if (at === null) return make('span', 'Unobserved', 'subtle');
  const el = make('time', timestamp(at), 'time');
  el.dataset.at = String(at);
  el.dateTime = new Date(at).toISOString();
  el.title = new Date(at).toLocaleString();
  return el;
}

function updateTimes() {
  for (const el of document.querySelectorAll('time[data-at]')) {
    el.textContent = timestamp(Number(el.dataset.at));
  }
}

function tag(label) {
  const el = make('span', label, 'tag');
  el.dataset.state = label.toLowerCase();
  return el;
}

function label(text, detail) {
  const el = make('div');
  el.append(make('span', text, 'label'), make('span', detail, 'id'));
  return el;
}

function table(destination, headings, rows, empty) {
  const root = $(destination);
  if (!rows.length) { root.replaceChildren(make('p', empty, 'empty')); return; }
  const wrap = make('div', undefined, 'table-wrap');
  wrap.tabIndex = 0;
  wrap.setAttribute('role', 'region');
  wrap.setAttribute('aria-label', headings[0] + ' table');
  const grid = make('table');
  const header = make('tr');
  for (const heading of headings) {
    const th = make('th', heading);
    th.scope = 'col';
    header.append(th);
  }
  const head = make('thead');
  head.append(header);
  const body = make('tbody');
  for (const row of rows) {
    const tr = make('tr');
    for (const value of row) {
      const cell = make('td');
      cell.append(value);
      tr.append(cell);
    }
    body.append(tr);
  }
  grid.append(head, body);
  wrap.append(grid);
  root.replaceChildren(wrap);
}

function entries(destination, rows, empty) {
  const root = $(destination);
  if (!rows.length) { root.replaceChildren(make('p', empty, 'empty')); return; }
  const list = make('ul', undefined, 'entries');
  for (const [name, detail, right] of rows) {
    const item = make('li', undefined, 'entry');
    item.append(label(name, detail), right);
    list.append(item);
  }
  root.replaceChildren(list);
}

function deviceName(id) {
  return snapshot.devices.find(device => device.deviceId === id)?.name || id;
}

function metric(name, value, detail, view) {
  const el = make('a', undefined, 'metric');
  el.href = '#' + view;
  el.append(make('span', name), make('strong', String(value)), make('small', detail));
  return el;
}

function overview() {
  const { devices, sessions, queue, receipts, cloudAgents, truncated } = snapshot;
  const count = (value, clipped) => clipped ? value + '+' : value;
  $('metrics').replaceChildren(
    metric('Registered devices', count(devices.length, truncated.devices), snapshot.source === 'fixture' ? 'Sample inventory' : 'Observed in Cloudflare', 'devices'),
    metric('Exposed sessions', count(sessions.length, truncated.sessions), 'Published by devices', 'devices'),
    metric('Cloud agents', cloudAgents?.length ?? '—', cloudAgents ? 'Hosted directory' : 'Directory unavailable', 'cloud'),
    metric('Queued messages', queue.queued, 'Waiting for delivery', 'activity'),
  );
  entries('device-summary', devices.slice(0, 5).map(d => [d.name || 'Unnamed device', d.deviceId, clock(d.lastSeen)]), 'No devices have registered.');
  entries('recent-receipts', receipts.slice(0, 5).map(r => [r.state[0].toUpperCase() + r.state.slice(1), deviceName(r.deviceId), clock(r.at)]), 'No signed receipts in this window.');
}

function devices() {
  const query = $('device-filter').value.trim().toLowerCase();
  const visible = snapshot.devices.filter(d => ((d.name || '') + ' ' + d.deviceId).toLowerCase().includes(query));
  table('device-list', ['Device', 'Last seen', 'Observation'],
    visible.map(d => [label(d.name || 'Unnamed device', d.deviceId), clock(d.lastSeen),
      make('span', d.lastSeen === null ? 'Unavailable' : 'Server-observed request', 'confidence')]),
    query ? 'No matching devices.' : 'No devices have registered.');
  table('session-list', ['Session', 'Device', 'Last exposure', 'Self-reported status'],
    snapshot.sessions.map(s => {
      const report = s.selfReported;
      const status = make('div');
      status.append(tag(report.status || 'Unavailable'));
      if (report.observedAt !== null) status.append(clock(report.observedAt));
      status.append(make('span', report.status ? 'Self-reported · unverified' : 'No report available', 'confidence'));
      return [label(s.harness, s.sessionId), make('span', deviceName(s.deviceId)), clock(s.lastSeen), status];
    }), 'No exposed sessions.');
}

function cloud() {
  const records = snapshot.cloudAgents;
  $('cloud-source').textContent = records ? 'Sample hosted identities' : 'Gateway directory';
  $('cloud-filter').disabled = records === null;
  const query = $('cloud-filter').value.trim().toLowerCase();
  const visible = (records || []).filter(row => [row.name, row.agentId, row.kind].join(' ').toLowerCase().includes(query));
  table('cloud-list', ['Agent', 'Kind', 'Source'],
    visible.map(row => [label(row.name, row.agentId), make('span', row.kind), tag('Fixture')]),
    records ? 'No matching cloud agents.' : 'The gateway’s hosted directory is not connected to this console.');
}

function activity() {
  const state = $('receipt-filter').value;
  const visible = snapshot.receipts.filter(r => state === 'all' || r.state === state);
  table('receipt-list', ['Receipt', 'Device', 'State', 'Observed'],
    visible.map(r => [make('span', r.messageId, 'id'), make('span', deviceName(r.deviceId)), tag(r.state), clock(r.at)]),
    state === 'all' ? 'No signed receipts in this window.' : 'No receipts in this state.');
}

function settings() {
  const fixture = snapshot.source === 'fixture';
  const fields = [
    ['Data source', fixture ? 'Fixture data' : 'Cloudflare'],
    ['Console origin', location.origin],
    ['Read scope', fixture ? 'No live grant' : 'mesh:read:v1'],
    ['Grant expiry', fixture ? 'No live grant' : new Date(snapshot.grantExpiresAt).toLocaleString()],
    ['Refresh', 'Every 15 seconds while visible'],
  ];
  $('connection').replaceChildren(...fields.map(([name, value]) => {
    const row = make('div', undefined, 'detail');
    row.append(make('dt', name), make('dd', value));
    return row;
  }));
  const link = $('account-link');
  link.hidden = !snapshot.accountUrl;
  if (snapshot.accountUrl) link.href = snapshot.accountUrl;
  else link.removeAttribute('href');
  $('permissions-note').textContent = fixture
    ? 'This preview uses sample data. To connect a mesh, request a read grant with the Everett setup CLI and approve it in account access.'
    : 'Dashboard access is read only. Manage or revoke the console grant in Everett account access. Tool permissions use the gateway’s owner policy.';
}

function render(data) {
  snapshot = data;
  approved = true;
  const fixture = data.source === 'fixture';
  source(fixture ? 'Fixture' : 'Cloudflare', data.source);
  notice(fixture ? 'Preview with sample data. Your devices and cloud agents are not connected.' : 'Read access approved by the account owner.', fixture ? 'fixture' : '');
  $('data').hidden = false;
  $('login').hidden = true;
  $('signout').hidden = false;
  $('refresh').hidden = false;
  drawView();
  $('snapshot-time').replaceChildren(fixture ? 'Fixture refreshed ' : 'Cloudflare refreshed ', clock(data.serverNow));
}

function drawView() {
  ({ overview, devices, cloud, activity, settings })[currentView]();
}

const privateResults = [...Object.values(viewResults).flat(), 'snapshot-time'];
function clear() {
  snapshot = null;
  $('data').hidden = true;
  for (const id of privateResults) $(id).replaceChildren();
  $('account-link').removeAttribute('href');
  $('account-link').hidden = true;
  source('Access not verified', 'unknown');
}

function cancelRefresh() {
  const request = pending;
  pending = null;
  request?.abort();
}

async function refresh() {
  if (pending || document.hidden) return;
  const ctrl = new AbortController();
  pending = ctrl;
  $('refresh').disabled = true;
  try {
    const response = await fetch('/api/mesh', {
      cache: 'no-store', signal: AbortSignal.any([ctrl.signal, AbortSignal.timeout(10000)]),
    });
    const data = response.ok ? await response.json() : null;
    if (pending !== ctrl || document.hidden) return;
    if (data) { render(data); return; }
    clear();
    if (response.status === 401) {
      approved = false;
      $('login').hidden = false;
      $('signout').hidden = true;
      $('refresh').hidden = true;
      notice('Sign in to view this private workspace.');
    } else if (response.status === 403) {
      notice('Read access ended. Mesh data has been cleared.', 'error');
    } else if (response.status === 404) {
      notice('The owner needs to enable this console and approve a read grant.');
    } else notice('The mesh source is unavailable. Data has been cleared.', 'error');
  } catch {
    if (pending === ctrl) {
      clear();
      notice('The mesh source is unavailable. Data has been cleared.', 'error');
    }
  } finally {
    if (pending === ctrl) {
      pending = null;
      $('refresh').disabled = false;
    }
  }
}

$('login-form').addEventListener('submit', async event => {
  event.preventDefault();
  const secret = $('secret').value;
  $('secret').value = '';
  try {
    const response = await fetch('/api/login', {
      method: 'POST', body: new URLSearchParams({ secret }), signal: AbortSignal.timeout(10000),
    });
    if (!response.ok) { notice('Sign-in failed. Check the console access secret.', 'error'); return; }
    cancelRefresh();
    await refresh();
  } catch { notice('Sign-in unavailable. Try again.', 'error'); }
});
$('signout').addEventListener('click', async () => {
  cancelRefresh();
  clear();
  approved = false;
  $('signout').hidden = true;
  $('refresh').hidden = true;
  try {
    const response = await fetch('/api/logout', { method: 'POST', signal: AbortSignal.timeout(10000) });
    if (!response.ok) throw new Error('Logout failed');
    notice('Signed out.');
  } catch { notice('Could not confirm sign-out. Close this console if the device is shared.', 'error'); }
  $('login').hidden = false;
});
$('refresh').addEventListener('click', refresh);
$('device-filter').addEventListener('input', () => { if (snapshot) devices(); });
$('cloud-filter').addEventListener('input', () => { if (snapshot) cloud(); });
$('receipt-filter').addEventListener('change', () => { if (snapshot) activity(); });
for (const key of Object.keys(prefs)) {
  $(key).addEventListener('change', event => {
    const value = event.target.value;
    if (choices[key].includes(value)) { prefs[key] = value; applyPrefs(true); }
  });
}
$('reset-prefs').addEventListener('click', () => { prefs = { ...defaultPrefs }; applyPrefs(true); });
systemTheme.addEventListener('change', () => applyPrefs());
window.addEventListener('hashchange', setView);
document.addEventListener('visibilitychange', () => {
  if (document.hidden) {
    cancelRefresh();
    clear();
  } else refresh();
});
setInterval(() => { if (approved) refresh(); }, 15000);
setInterval(updateTimes, 5000);
applyPrefs();
setView();
refresh();
