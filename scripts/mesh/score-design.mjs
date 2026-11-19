import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { once } from 'node:events';
import assert from 'node:assert/strict';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.MESH_PLAYWRIGHT || 'playwright-core');
const port = 18879;
const origin = 'http://127.0.0.1:' + port;
const secret = 'mesh-console-visual-fixture-only';
const server = spawn(process.execPath, ['web-mesh/dev.mjs'], {
  cwd: new URL('../../', import.meta.url),
  env: {...process.env, ENABLED:'true', MESH_MODE:'fixture', CONSOLE_ORIGIN:origin,
    PORT:String(port), CONSOLE_SECRET:secret, SESSION_KEY:'k'.repeat(64)},
});
let browser;
let failures = 0;
let count = 0;
try {
  await Promise.race([once(server.stdout, 'data'), once(server, 'exit').then(() => {
    throw Error('Fixture console failed to start');
  })]);
  browser = await chromium.launch({executablePath: process.env.MESH_CHROMIUM});
  const page = await browser.newPage({viewport:{width:1440,height:900}});
  page.setDefaultTimeout(1500);
  await page.goto(origin);
  await page.locator('#secret').fill(secret);
  await page.getByRole('button',{name:'Sign in'}).click();
  await page.locator('#data').waitFor({state:'visible'});

  async function scenario(label, check) {
    count++;
    try {await check(); console.log('PASS '+label);}
    catch {failures++; console.log('UNMET '+label);}
  }
  await scenario('overview leads to device inventory, activity and setup', async () => {
    const nav=page.getByRole('navigation',{name:'Console'});
    for (const label of ['Overview','Devices','Cloud agents','Activity','Settings']) {
      await nav.getByRole('link',{name:label,exact:true}).waitFor();
    }
    assert.match(await page.locator('#source').innerText(),/Sample data/);
  });
  await scenario('device search narrows the enrolled device list', async () => {
    await page.goto(origin+'/#devices');
    await page.getByRole('heading',{name:'Devices',exact:true}).waitFor();
    await page.getByRole('searchbox',{name:'Filter devices'}).fill('fixture-mac');
    await page.getByText('fixture-mac-01',{exact:true}).waitFor();
    assert.equal(await page.getByText('fixture-nuc-02',{exact:true}).count(),0);
  });
  await scenario('cloud agents have a distinct source and fixture identities', async () => {
    await page.goto(origin+'/#cloud');
    await page.getByRole('heading',{name:'Cloud agents',exact:true}).waitFor();
    await page.getByText('Demo Codex',{exact:true}).waitFor();
    await page.getByText('Demo ChatGPT',{exact:true}).waitFor();
    await page.getByText('Sample hosted identities',{exact:true}).waitFor();
    assert.equal(await page.getByText('fixture-session-codex',{exact:true}).count(),0);
  });
  await scenario('settings persist theme, density and time format', async () => {
    await page.goto(origin+'/#settings');
    assert.equal(await page.getByLabel('Theme',{exact:true}).inputValue(),'light');
    await page.getByLabel('Theme',{exact:true}).selectOption('dark');
    await page.getByLabel('Table density').selectOption('compact');
    await page.getByLabel('Time format').selectOption('absolute');
    await page.reload();
    assert.equal(await page.getByLabel('Theme',{exact:true}).inputValue(),'dark');
    assert.equal(await page.getByLabel('Table density').inputValue(),'compact');
    assert.equal(await page.getByLabel('Time format').inputValue(),'absolute');
    await page.getByRole('heading',{name:'Connection and access'}).waitFor();
    assert.equal(await page.getByRole('button',{name:'Save permissions'}).count(),0);
  });
  await scenario('mobile navigation and device view fit the viewport', async () => {
    await page.setViewportSize({width:390,height:844});
    await page.goto(origin+'/#devices');
    await page.getByRole('navigation',{name:'Console'}).getByRole('link',{name:'Devices',exact:true}).waitFor();
    await page.getByText('fixture-mac-01',{exact:true}).waitFor();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),true);
  });
  await scenario('visibility refresh cannot repopulate the dashboard during sign-out', async () => {
    const logoutStarted = Promise.withResolvers();
    const finishLogout = Promise.withResolvers();
    const loggedOut = Promise.withResolvers();
    let reads = 0;
    page.on('request', request => {
      if (new URL(request.url()).pathname === '/api/mesh') reads++;
    });
    await page.route('**/api/logout', async route => {
      logoutStarted.resolve();
      await finishLogout.promise;
      await route.continue();
    });
    page.on('response', response => {
      if (new URL(response.url()).pathname === '/api/logout') loggedOut.resolve(response.status());
    });
    await page.getByRole('button', { name: 'Sign out' }).click();
    await logoutStarted.promise;
    try {
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      await page.locator('#data').waitFor({ state: 'hidden' });
      // A page round trip drains the event before the deferred logout response.
      assert.equal(await page.evaluate(() => document.hidden), false);
      assert.equal(reads, 0);
    } finally {
      finishLogout.resolve();
    }
    assert.equal(await loggedOut.promise, 204);
    await page.locator('#secret').fill(secret);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.locator('#data').waitFor({ state: 'visible' });
  });
  await scenario('a fresh authenticated view keeps refresh and sign-out available after grant denial', async () => {
    let revoked = true;
    await page.route('**/api/mesh', async route => {
      if (revoked) await route.fulfill({
        status: 403, json: { error: 'mesh_access_revoked', authentication: 'github' },
      });
      else await route.continue();
    });
    await page.clock.install();
    await page.goto(origin + '/everett/dashboard#devices');
    await page.getByText('Mesh read access ended.', { exact: false }).waitFor();
    await page.getByRole('button', { name: 'Refresh' }).waitFor();
    await page.getByRole('button', { name: 'Sign out' }).waitFor();
    assert.equal(await page.locator('#data').isVisible(), false);
    assert.equal(await page.getByText('fixture-mac-01', { exact: true }).count(), 0);
    revoked = false;
    await page.clock.fastForward(15000);
    await page.getByText('fixture-mac-01', { exact: true }).waitFor();
  });
  assert.equal(count,7);
  console.log('unmet='+failures);
} finally {
  await browser?.close();
  server.kill();
}
