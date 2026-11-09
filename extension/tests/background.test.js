// Runs the real background.js in a vm with a fake chrome.* and a scriptable native host.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const EXT = path.join(__dirname, '..');
const tick = () => new Promise((r) => setImmediate(r));

function makeEnv(hostBehavior, { enabled = true, skip = [] } = {}) {
  const L = {};
  const posted = [];
  const cancelled = [];
  const erased = [];
  const sent = [];
  const port = {
    postMessage(m) {
      posted.push(m);
      setImmediate(() => hostBehavior(m, (reply) => port._onMsg(reply)));
    },
    onMessage: { addListener: (f) => { port._onMsg = f; } },
    onDisconnect: { addListener: (f) => { port._onDisc = f; } },
  };
  const chrome = {
    runtime: {
      id: 'self-id', lastError: null,
      connectNative: () => port,
      onMessage: { addListener: (f) => { L.msg = f; } },
      onStartup: { addListener() {} }, onInstalled: { addListener() {} },
    },
    action: { setBadgeBackgroundColor() {}, setBadgeText() {}, setTitle() {} },
    storage: {
      local: { get: async (d) => ({ ...d, enabled, skip }), set: async () => {} },
      session: { get: async (d) => d, set: async () => {} },
    },
    cookies: { getAll: async () => [{ name: 'sid', value: 'abc' }, { name: 'k', value: 'v' }] },
    downloads: {
      onCreated: { addListener: (f) => { L.dl = f; } },
      cancel: async (id) => { cancelled.push(id); },
      erase: async (o) => { erased.push(o.id); },
    },
    webRequest: { onHeadersReceived: { addListener: (f) => { L.headers = f; } } },
    tabs: {
      onRemoved: { addListener() {} }, onUpdated: { addListener() {} },
      sendMessage: async (id, m) => { sent.push([id, m]); },
      get: async () => ({ url: 'https://page.test/watch' }),
    },
    alarms: { create() {}, onAlarm: { addListener: (f) => { L.alarm = f; } } },
  };
  const ctx = vm.createContext({
    chrome, navigator: { userAgent: 'TestUA/1' }, console, setTimeout, clearTimeout, URL, setImmediate,
  });
  ctx.self = ctx;
  ctx.importScripts = (f) => vm.runInContext(fs.readFileSync(path.join(EXT, f), 'utf8'), ctx);
  vm.runInContext(fs.readFileSync(path.join(EXT, 'background.js'), 'utf8'), ctx);
  const ask = (msg, tabId) => new Promise((resolve) => {
    const keep = L.msg(msg, tabId == null ? {} : { tab: { id: tabId } }, resolve);
    if (keep !== true && keep !== undefined) { /* sync response already delivered */ }
  });
  return { L, posted, cancelled, erased, sent, port, ask };
}

const okHost = (m, reply) => {
  if (m.command === 'ping') return reply({ status: 'ok', id: m.id, data: { pong: true } });
  if (m.command === 'browser.intercept' || m.command === 'media.send_to_app') {
    return reply({ status: 'ok', id: m.id, data: { request_id: 'prop_1', state: 'pending_confirmation' } });
  }
  if (m.command === 'download.list') return reply({ status: 'ok', id: m.id, data: { items: [] } });
};

const downHost = (m, reply) => reply({ status: 'error', id: m.id, error: { code: 'ERR_NATIVE_HOST_NOT_FOUND', message: 'UnduhKilat tidak aktif' } });

const item = (o = {}) => ({ id: 42, url: 'https://files.test/a.zip', finalUrl: 'https://cdn.test/a.zip', referrer: 'https://files.test/page', filename: 'C:\\Users\\x\\Downloads\\a.zip', mime: 'application/zip', totalBytes: 1234, fileSize: 1234, state: 'in_progress', startTime: new Date().toISOString(), ...o });

test('accepted browser download is handed over with cookies/referer/UA, then cancelled and erased', async () => {
  const e = makeEnv(okHost);
  await tick();
  await e.L.dl(item());
  const req = e.posted.find((m) => m.command === 'browser.intercept');
  assert.ok(req, 'browser.intercept was sent');
  assert.equal(req.payload.url, 'https://cdn.test/a.zip', 'final URL after redirects');
  assert.equal(req.payload.cookies, 'sid=abc; k=v');
  assert.equal(req.payload.referer, 'https://files.test/page');
  assert.equal(req.payload.user_agent, 'TestUA/1');
  assert.equal(req.payload.file_name, 'a.zip');
  assert.equal(req.payload.total_bytes, 1234);
  assert.deepEqual(e.cancelled, [42]);
  assert.deepEqual(e.erased, [42]);
});

test('if the app is not reachable the browser download is NOT cancelled', async () => {
  const e = makeEnv(downHost);
  await tick();
  await e.L.dl(item());
  assert.deepEqual(e.cancelled, [], 'capture must never make a download disappear');
  assert.deepEqual(e.erased, []);
});

test('if the host times out or disconnects the browser download continues', async () => {
  const e = makeEnv(() => { /* never answers */ });
  await tick();
  const p = e.L.dl(item());
  await tick();
  e.port._onDisc(); // browser tears the port down
  await p;
  assert.deepEqual(e.cancelled, []);
});

test('capture can be switched off', async () => {
  const e = makeEnv(okHost, { enabled: false });
  await tick(); await tick();
  await e.L.dl(item());
  assert.ok(!e.posted.some((m) => m.command === 'browser.intercept'));
  assert.deepEqual(e.cancelled, []);
});

test('blob/data URLs and extension-initiated downloads are left alone', async () => {
  const e = makeEnv(okHost);
  await tick();
  await e.L.dl(item({ url: 'blob:https://x/1', finalUrl: undefined }));
  await e.L.dl(item({ url: 'data:text/plain,hi', finalUrl: undefined }));
  await e.L.dl(item({ byExtensionId: 'someone-else' }));
  assert.ok(!e.posted.some((m) => m.command === 'browser.intercept'));
  assert.deepEqual(e.cancelled, []);
});

test('old downloads replayed from browser history at startup are NOT handed over or erased', async () => {
  const e = makeEnv(okHost);
  await tick();
  const old = new Date(Date.now() - 3 * 24 * 3600 * 1000).toISOString();
  await e.L.dl(item({ id: 1, state: 'complete', startTime: old }));
  await e.L.dl(item({ id: 2, state: 'interrupted', startTime: old }));
  await e.L.dl(item({ id: 3, state: 'in_progress', startTime: old })); // auto-resumed old download
  assert.ok(!e.posted.some((m) => m.command === 'browser.intercept'));
  assert.deepEqual(e.cancelled, []);
  assert.deepEqual(e.erased, [], 'history entries must never be erased');
});

test('sniffed media becomes one deduplicated candidate with its size and is pushed to the tab', async () => {
  const e = makeEnv(okHost);
  await tick();
  const hdr = (n, v) => ({ name: n, value: v });
  e.L.headers({ tabId: 7, url: 'https://cdn.test/v.mp4?range=0-99', type: 'media', statusCode: 206, responseHeaders: [hdr('Content-Type', 'video/mp4'), hdr('Content-Range', 'bytes 0-99/5000000')] });
  e.L.headers({ tabId: 7, url: 'https://cdn.test/v.mp4?range=100-199', type: 'media', statusCode: 206, responseHeaders: [hdr('Content-Type', 'video/mp4'), hdr('Content-Range', 'bytes 100-199/5000000')] });
  e.L.headers({ tabId: 7, url: 'https://cdn.test/app.js', type: 'script', statusCode: 200, responseHeaders: [hdr('Content-Type', 'text/javascript')] });
  e.L.headers({ tabId: -1, url: 'https://cdn.test/bg.mp4', type: 'media', statusCode: 200, responseHeaders: [hdr('Content-Type', 'video/mp4')] });
  const r = await e.ask({ type: 'get-candidates' }, 7);
  assert.equal(r.list.length, 1);
  assert.equal(r.list[0].size, 5000000);
  assert.deepEqual([...r.list[0].sources], ['network']); // copy: arrays from the vm realm differ in prototype
  assert.ok(e.sent.some(([id, m]) => id === 7 && m.type === 'candidates'));
});

test('DOM-hook and network candidates are unioned; HLS is listed but cannot be sent', async () => {
  const e = makeEnv(okHost);
  await tick();
  e.L.headers({ tabId: 3, url: 'https://cdn.test/v.mp4', type: 'media', statusCode: 200, responseHeaders: [{ name: 'content-type', value: 'video/mp4' }, { name: 'content-length', value: '777000' }] });
  await e.ask({ type: 'dom-media', items: [{ url: 'https://cdn.test/v.mp4' }, { url: 'https://cdn.test/live.m3u8' }, { url: 'blob:https://cdn.test/zzz' }] }, 3);
  const list = (await e.ask({ type: 'get-candidates' }, 3)).list;
  assert.equal(list.length, 2, 'blob: ignored, mp4 deduped across sources');
  const mp4 = list.find((c) => c.label === 'MP4');
  assert.deepEqual([...mp4.sources].sort(), ['network', 'page']);
  const hls = list.find((c) => c.label === 'HLS');
  assert.equal(hls.kind, 'manifest');

  const refused = await e.ask({ type: 'send-media', url: 'https://cdn.test/live.m3u8' }, 3);
  assert.equal(refused.ok, false);
  assert.ok(!e.posted.some((m) => m.command === 'media.send_to_app'), 'no broken manifest download is ever requested');

  const sentOk = await e.ask({ type: 'send-media', url: 'https://cdn.test/v.mp4' }, 3);
  assert.equal(sentOk.ok, true);
  const req = e.posted.find((m) => m.command === 'media.send_to_app');
  assert.equal(req.payload.referer, 'https://page.test/watch');
  assert.equal(req.payload.total_bytes, 777000);
  assert.equal(req.payload.cookies, 'sid=abc; k=v');
});

test('send-media reports the "not running" message when the app is down', async () => {
  const e = makeEnv(downHost);
  await tick();
  e.L.headers({ tabId: 5, url: 'https://cdn.test/a.mp4', type: 'media', statusCode: 200, responseHeaders: [{ name: 'content-type', value: 'video/mp4' }] });
  const r = await e.ask({ type: 'send-media', url: 'https://cdn.test/a.mp4' }, 5);
  assert.equal(r.ok, false);
  assert.match(r.error, /UnduhKilat tidak aktif/);
});

test('pushed status updates the popup list using the engine state names', async () => {
  const e = makeEnv(okHost);
  await tick();
  e.port._onMsg({ type: 'event', event: 'browser.proposal_result', payload: { request_id: 'prop_1', accepted: true, download_id: 'dl-1' } });
  e.port._onMsg({ type: 'event', event: 'browser.push_status', payload: { request_id: 'prop_1', download_id: 'dl-1', state: 'running', downloaded_bytes: 50, total_bytes: 100, file_name: 'a.zip' } });
  let s = await e.ask({ type: 'popup-state' });
  assert.equal(s.tracked.length, 1);
  assert.equal(s.tracked[0].state, 'running');
  assert.equal(s.tracked[0].fileName, 'a.zip');
  e.port._onMsg({ type: 'event', event: 'browser.push_status', payload: { request_id: 'prop_1', download_id: 'dl-1', state: 'completed', downloaded_bytes: 100, total_bytes: 100 } });
  s = await e.ask({ type: 'popup-state' });
  assert.equal(s.tracked[0].state, 'completed');
});

test('tiny media (UI sounds) is not offered as a candidate', async () => {
  const e = makeEnv(okHost);
  await tick();
  e.L.headers({ tabId: 9, url: 'https://www.youtube.com/s/sound/click.mp3', type: 'media', statusCode: 200, responseHeaders: [{ name: 'content-type', value: 'audio/mpeg' }, { name: 'content-length', value: '6800' }] });
  const r = await e.ask({ type: 'get-candidates' }, 9);
  assert.equal(r.list.length, 0);
});

const skipPdfHost = (m, reply) => {
  if (m.command === 'ping') return reply({ status: 'ok', id: m.id, data: { pong: true, skip_extensions: ['pdf'] } });
  return okHost(m, reply);
};

test('types excluded by the app (sent with ping) stay in the browser; others are still captured', async () => {
  const e = makeEnv(skipPdfHost);
  await tick(); await tick();
  await e.L.dl(item({ id: 7, url: 'https://www.tokopedia.com/digital/invoice?id=1', finalUrl: '', filename: '', mime: 'application/pdf' }));
  assert.equal(e.posted.filter((m) => m.command === 'browser.intercept').length, 0);
  assert.deepEqual(e.cancelled, []);
  await e.L.dl(item());
  assert.equal(e.posted.filter((m) => m.command === 'browser.intercept').length, 1);
  assert.deepEqual(e.cancelled, [42]);
});

test('cached exclusion list applies while the app is down (no host call, app not started)', async () => {
  const e = makeEnv(downHost, { skip: ['pdf'] });
  await tick(); await tick();
  await e.L.dl(item({ url: 'https://a.test/doc.pdf', finalUrl: '', filename: '', mime: '' }));
  assert.equal(e.posted.filter((m) => m.command === 'browser.intercept').length, 0);
});
