import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.MESH_PLAYWRIGHT || 'playwright-core');
const width = process.argv.includes('--holdout') ? 320 : 390;
const port = 18885;
const origin = 'http://127.0.0.1:' + port;
const password = 'design-fixture-login-local-only-20261119';
const server = spawn(process.execPath, ['web-mesh/dev.mjs'], {
  cwd: new URL('../../', import.meta.url),
  env: { ...process.env, ENABLED:'true', MESH_MODE:'fixture', CONSOLE_ORIGIN:origin,
    PORT:String(port), CONSOLE_SECRET:password, SESSION_KEY:'s'.repeat(64) },
});
let browser;
let unmet = 0;
async function check(name, run) {
  try { await run(); console.log('PASS', name); }
  catch (error) { unmet++; console.log('UNMET', name, error.message.split('\n')[0]); }
}

try {
  await Promise.race([once(server.stdout, 'data'), once(server, 'exit').then(() => { throw Error('fixture start failed'); })]);
  browser = await chromium.launch({ executablePath: process.env.MESH_CHROMIUM });
  const page = await browser.newPage({ viewport:{width:1440,height:900}, reducedMotion:'reduce' });
  page.setDefaultTimeout(1200);
  await page.goto(origin + '/everett/dashboard');
  await page.locator('#secret').fill(password);
  await page.getByRole('button',{name:'Sign in',exact:true}).click();
  await page.locator('#data').waitFor();
  assert.match(await page.locator('#source').innerText(), /Sample data/);

  if (process.env.MESH_SHOTS) await page.screenshot({path:process.env.MESH_SHOTS+'-overview.png', fullPage:true});
  await check('overview has one compact metric band with distinct linked segments', async () => {
    const band = page.locator('#metrics');
    const bandStyle = await band.evaluate(el => {
      const css=getComputedStyle(el), child=getComputedStyle(el.firstElementChild);
      return { height:el.getBoundingClientRect().height, border:parseFloat(css.borderTopWidth), childCorners:parseFloat(child.borderTopLeftRadius), gap:parseFloat(css.columnGap) };
    });
    assert.equal(await band.getByRole('link').count(),4);
    assert.ok(bandStyle.height <= 102);
    assert.ok(bandStyle.border >= 1 && bandStyle.childCorners === 0 && bandStyle.gap === 0);
  });
  await check('overview puts exact exposed sessions and their devices in the primary panel', async () => {
    const session = page.getByText('fixture-session-codex',{exact:true});
    await session.waitFor();
    const row = page.getByRole('listitem').filter({has:session});
    assert.match(await row.innerText(), /Mac · fixture/);
    const title = await page.getByRole('heading',{name:'Exposed sessions',exact:true}).boundingBox();
    const timeline = await page.getByRole('heading',{name:'Recent receipts',exact:true}).boundingBox();
    assert.ok(title && timeline && title.x < timeline.x && Math.abs(title.y - timeline.y) < 24);
  });

  await page.goto(origin + '/everett/dashboard#devices');
  await page.getByText('fixture-mac-01',{exact:true}).waitFor();
  if (process.env.MESH_SHOTS) await page.screenshot({path:process.env.MESH_SHOTS+'-devices.png', fullPage:true});
  await check('filter narrows exposed sessions alongside their registered devices', async () => {
    const filter = page.getByRole('searchbox',{name:'Filter devices'});
    await filter.fill('fixture-nuc');
    assert.equal(await page.getByText('fixture-session-claude',{exact:true}).count(), 1);
    assert.equal(await page.getByText('fixture-session-codex',{exact:true}).count(), 0);
  });
  await page.getByRole('searchbox',{name:'Filter devices'}).fill('');
  await page.setViewportSize({width,height:820});
  await page.getByRole('searchbox',{name:'Filter devices'}).blur();
  if (process.env.MESH_SHOTS) await page.screenshot({path:process.env.MESH_SHOTS+'-mobile.png', fullPage:true});
  await check('all mobile nav links are visible and have 44px vertical touch targets', async () => {
    const links = page.getByRole('navigation',{name:'Console'}).getByRole('link');
    assert.equal(await links.count(),5);
    const fit = await links.evaluateAll(items => items.every(el => {
      const r = el.getBoundingClientRect();
      return r.left >= 0 && r.right <= innerWidth && r.height >= 44 && el.scrollWidth <= el.clientWidth;
    }));
    assert.ok(fit);
  });
  await check('phone-width search shows the full control on its own line', async () => {
    const filter = page.getByRole('searchbox',{name:'Filter devices'});
    const heading = page.getByRole('heading',{name:'Registered devices'});
    const outer = await page.locator('[data-view="devices"] .panel-heading').first().boundingBox();
    const input = await filter.boundingBox();
    const title = await heading.boundingBox();
    assert.ok(outer && title && input);
    assert.ok(input.y >= title.y + title.height && input.width >= outer.width * 0.85);
  });
  await check('mobile session detail fits without sideways scrolling', async () => {
    const table = page.getByRole('table').last();
    assert.match(await table.innerText(),/fixture-session-codex/);
    const fit = await table.evaluate(el => {
      const region=el.closest('.table-wrap');
      return el.scrollWidth <= region.clientWidth && document.documentElement.scrollWidth <= innerWidth;
    });
    assert.ok(fit);
  });

  await check('skip link focuses the current device view', async () => {
    const skip = page.getByRole('link',{name:'Skip to content'});
    await skip.focus();
    await skip.press('Enter');
    assert.equal(new URL(page.url()).hash, '#devices');
    assert.equal(await page.locator('main').evaluate(el => document.activeElement === el), true);
    assert.equal(await page.getByRole('heading',{name:'Devices',exact:true}).count(), 1);
  });
  await page.goto(origin + '/everett/dashboard#settings');
  await check('phone settings values sit below their labels', async () => {
    const label = await page.getByText('Console origin',{exact:true}).boundingBox();
    const value = await page.locator('#connection dd').filter({hasText:origin}).boundingBox();
    assert.ok(label && value && value.y >= label.y + label.height);
  });

  await page.setViewportSize({width:1440,height:900});
  const longHarness = 'h'.repeat(160);
  let revisedSnapshot = false;
  let queued = 2;
  await page.clock.install();
  await page.route('**/api/mesh', async route => {
    const upstream = await route.fetch();
    const sample = await upstream.json();
    sample.sessions[0].harness = longHarness;
    if (revisedSnapshot) sample.devices[0].name = 'Mac after refresh';
    sample.queue.queued = queued;
    await route.fulfill({json:{ ...sample, source:'cloudflare', cloudAgents:null }});
  });
  await page.goto(origin+'/everett/dashboard');
  await page.reload();
  await page.getByText('Cloudflare',{exact:true}).first().waitFor();
  await check('a healthy Cloudflare read does not occupy a second status banner', async () => {
    assert.equal(await page.locator('#notice').isVisible(), false);
    assert.equal(await page.locator('#data').isVisible(), true);
  });
  await check('a maximum-length session harness stays inside the phone overview', async () => {
    await page.setViewportSize({width,height:820});
    const label = page.getByText(longHarness,{exact:true});
    await label.waitFor();
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    const row = label.locator('xpath=ancestor::li');
    const text = await label.boundingBox();
    const meta = await row.locator('.entry-meta').boundingBox();
    assert.ok(text && meta && text.x + text.width <= meta.x);
  });
  await page.goto(origin + '/everett/dashboard#devices');
  await check('automatic refresh retains keyboard focus on the device table', async () => {
    const region = page.getByRole('region',{name:'Device table'});
    await region.focus();
    revisedSnapshot = true;
    await page.clock.fastForward(15000);
    await page.getByText('Mac after refresh',{exact:true}).first().waitFor();
    assert.equal(await region.evaluate(el => document.activeElement === el), true);
  });
  await page.goto(origin + '/everett/dashboard#overview');
  await check('automatic refresh updates metrics without dropping keyboard focus', async () => {
    const queue = page.getByRole('link',{name:/Queued messages/});
    await queue.focus();
    queued = 3;
    await queue.getByText('2',{exact:true}).waitFor();
    await page.clock.fastForward(15000);
    await queue.getByText('3',{exact:true}).waitFor();
    assert.equal(await queue.evaluate(el => document.activeElement === el), true);
  });
  console.log('unmet='+unmet);
} finally {
  await browser?.close();
  server.kill();
}
