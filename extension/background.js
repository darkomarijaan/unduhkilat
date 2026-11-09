// UnduhKilat service worker (MV3).
//
// Rules that matter:
//  * A browser download is only cancelled AFTER the desktop app accepted the proposal.
//    If the host or app is unreachable the browser keeps downloading (capture must never
//    make a download disappear).
//  * Nothing is downloaded by UnduhKilat until the user confirms in the app (FR-039).
/* global importScripts, UK */
importScripts('shared.js');

const HOST = 'com.unduhkilat.host';
const CALL_TIMEOUT_MS = 15000;
const ACTIVE_STATES = ['queued', 'running'];

let port = null;
let seq = 0;
const waiting = new Map(); // request id -> { resolve, reject, timer }
let connected = false;

// tabId -> Map(normalizedUrl -> candidate)
const tabCandidates = new Map();
// download id -> { requestId, fileName, state, downloaded, total }
let tracked = new Map();
let settings = { enabled: true, skip: [] };

// ---- storage (the worker is torn down after ~30 s idle; keep what matters) -------------

async function loadState() {
  try {
    const s = await chrome.storage.local.get({ enabled: true, skip: [] });
    settings.enabled = s.enabled !== false;
    settings.skip = UK.parseSkipList(s.skip);
    const t = await chrome.storage.session.get({ tracked: [] });
    tracked = new Map(t.tracked);
  } catch (e) { /* storage unavailable: run with defaults */ }
}

// The app sends its "jenis berkas yang tidak ditangkap" list with every ping; cache it so
// excluded downloads are left alone even while the app is closed (and never start it).
function applyPing(data) {
  if (!data || !Array.isArray(data.skip_extensions)) return;
  settings.skip = UK.parseSkipList(data.skip_extensions);
  try { chrome.storage.local.set({ skip: settings.skip }); } catch (e) { /* ignore */ }
}

function saveTracked() {
  try { chrome.storage.session.set({ tracked: [...tracked.entries()] }); } catch (e) { /* ignore */ }
}

// ---- native messaging --------------------------------------------------------------------

function setConnected(v) {
  if (connected === v) return;
  connected = v;
  updateBadge();
}

function updateBadge() {
  const active = [...tracked.values()].filter((t) => ACTIVE_STATES.includes(t.state)).length;
  chrome.action.setBadgeBackgroundColor({ color: connected ? '#00D2FF' : '#5E6A7E' });
  chrome.action.setBadgeText({ text: connected ? (active ? String(active) : '') : '!' });
  chrome.action.setTitle({ title: connected ? 'UnduhKilat' : 'UnduhKilat tidak aktif. Jalankan aplikasi portabel.' });
}

function failAll(error) {
  for (const [id, w] of waiting) {
    clearTimeout(w.timer);
    w.reject(error);
    waiting.delete(id);
  }
}

function connect() {
  if (port) return port;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (e) {
    port = null;
    return null;
  }
  port.onMessage.addListener(onHostMessage);
  port.onDisconnect.addListener(() => {
    const msg = chrome.runtime.lastError && chrome.runtime.lastError.message;
    port = null;
    setConnected(false);
    failAll({ code: 'ERR_NATIVE_HOST_NOT_FOUND', message: msg || 'native host disconnected' });
  });
  return port;
}

function call(command, payload, timeoutMs) {
  return new Promise((resolve, reject) => {
    const p = connect();
    if (!p) return reject({ code: 'ERR_NATIVE_HOST_NOT_FOUND', message: 'cannot start native host' });
    const id = 'req_' + Date.now().toString(36) + '_' + (++seq);
    const timer = setTimeout(() => {
      waiting.delete(id);
      reject({ code: 'ERR_NATIVE_HOST_TIMEOUT', message: 'no answer from UnduhKilat' });
    }, timeoutMs || CALL_TIMEOUT_MS);
    waiting.set(id, { resolve, reject, timer });
    try {
      p.postMessage({ id, command, payload: payload || {} });
    } catch (e) {
      clearTimeout(timer);
      waiting.delete(id);
      reject({ code: 'ERR_NATIVE_HOST_NOT_FOUND', message: String(e) });
    }
  });
}

function onHostMessage(m) {
  if (m && m.type === 'event') return onEvent(m);
  const w = m && waiting.get(m.id);
  if (!w) return;
  clearTimeout(w.timer);
  waiting.delete(m.id);
  if (m.status === 'ok') {
    setConnected(true);
    w.resolve(m.data);
  } else {
    // The host answers errors itself when the app is down; that is "not connected".
    if (m.error && m.error.code === 'ERR_NATIVE_HOST_NOT_FOUND') setConnected(false);
    w.reject(m.error || { code: 'ERR_INTERNAL', message: 'unknown error' });
  }
}

// ---- status push-back (FR-038) ---------------------------------------------------------------

function onEvent(ev) {
  const p = ev.payload || {};
  if (ev.event === 'browser.push_status') {
    const prev = tracked.get(p.download_id) || { requestId: p.request_id, fileName: p.file_name || '', downloaded: 0, total: -1 };
    tracked.set(p.download_id, {
      ...prev,
      state: p.state || prev.state,
      fileName: p.file_name || prev.fileName,
      downloaded: p.downloaded_bytes != null ? p.downloaded_bytes : prev.downloaded,
      total: p.total_bytes != null ? p.total_bytes : prev.total,
      error: p.error_code || null,
    });
    saveTracked();
    updateBadge();
  } else if (ev.event === 'browser.proposal_result' && p.accepted && p.download_id) {
    if (!tracked.has(p.download_id)) tracked.set(p.download_id, { requestId: p.request_id, fileName: '', state: 'queued', downloaded: 0, total: -1 });
    saveTracked();
  }
}

// After the worker slept, native pushes may have been missed: re-query via download.list.
async function requery() {
  if (!tracked.size) return;
  try {
    const data = await call('download.list', { limit: 100 });
    for (const it of data.items || []) {
      const t = tracked.get(it.id);
      if (t) tracked.set(it.id, { ...t, state: it.state, fileName: it.file_name || t.fileName, downloaded: it.downloaded_bytes, total: it.total_bytes, error: it.error_code || null });
    }
    saveTracked();
    updateBadge();
  } catch (e) { /* app not running */ }
}

// ---- browser download interception (FR-036/037) ---------------------------------------------

async function cookiesFor(url) {
  try {
    return UK.cookieHeader(await chrome.cookies.getAll({ url }));
  } catch (e) {
    return '';
  }
}

async function handOver(command, fields) {
  return call(command, fields);
}

async function onDownloadCreated(item) {
  if (!settings.enabled) return;
  if (item.byExtensionId) return; // started by an extension (including ours): leave it alone
  if (!UK.isFreshDownload(item)) return; // history replayed at browser startup: leave it alone
  const url = item.finalUrl || item.url;
  if (!UK.isHttp(url)) return; // blob:, data:, file: cannot be handed over
  if (UK.shouldSkipCapture(settings.skip, UK.baseName(item.filename), url, item.mime)) return; // user wants the browser to keep this type

  let accepted = false;
  try {
    const cookies = await cookiesFor(url);
    await handOver('browser.intercept', {
      url,
      file_name: UK.baseName(item.filename) || UK.fileNameFromUrl(url),
      mime_type: item.mime || '',
      total_bytes: item.totalBytes > 0 ? item.totalBytes : item.fileSize > 0 ? item.fileSize : null,
      referer: item.referrer || '',
      user_agent: (typeof navigator !== 'undefined' && navigator.userAgent) || '',
      cookies,
    });
    accepted = true;
  } catch (e) {
    // Host/app unreachable or request refused: the browser download simply continues.
    return;
  }
  if (accepted) {
    try {
      await chrome.downloads.cancel(item.id);
      await chrome.downloads.erase({ id: item.id });
    } catch (e) { /* already finished or gone */ }
  }
}

// ---- media candidates (DEC-003, FR-041/042) ----------------------------------------------------

function candidatesFor(tabId) {
  let m = tabCandidates.get(tabId);
  if (!m) { m = new Map(); tabCandidates.set(tabId, m); }
  return m;
}

function publish(tabId) {
  const list = [...candidatesFor(tabId).values()];
  chrome.tabs.sendMessage(tabId, { type: 'candidates', list }).catch(() => {});
}

function onHeaders(d) {
  if (d.tabId < 0 || !UK.isHttp(d.url)) return;
  const h = UK.headerMap(d.responseHeaders);
  const mime = (h['content-type'] || '').split(';')[0].trim();
  if (!UK.isMediaResponse(d.type, mime, d.url)) return;
  if (d.statusCode !== 200 && d.statusCode !== 206) return;
  const size = UK.sizeFromHeaders(h);
  if (UK.isTinyMedia(mime, d.url, size)) return; // e.g. YouTube's own UI sounds
  UK.mergeCandidate(candidatesFor(d.tabId), { url: d.url, mime, size, sources: ['network'] });
  publish(d.tabId);
}

// ---- messages from content script / popup ----------------------------------------------------------

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  const tabId = sender.tab ? sender.tab.id : msg.tabId;
  if (msg.type === 'dom-media' && tabId != null) {
    for (const it of msg.items || []) UK.mergeCandidate(candidatesFor(tabId), { url: it.url, mime: it.mime, size: null, sources: ['page'] });
    publish(tabId);
    sendResponse({ ok: true });
  } else if (msg.type === 'get-candidates') {
    sendResponse({ list: [...candidatesFor(tabId).values()], connected });
  } else if (msg.type === 'send-media') {
    (async () => {
      const key = UK.normalizeUrl(msg.url);
      if (!key) return sendResponse({ ok: false, error: 'Alamat video tidak valid.' });
      let c = candidatesFor(tabId).get(key);
      if (!c) {
        // Service worker MV3 tidur dan kehilangan peta kandidat, padahal panel di halaman masih
        // menampilkannya: pulihkan dari data yang dikirim panel, lalu klasifikasi ulang.
        c = UK.mergeCandidate(candidatesFor(tabId), { url: msg.url, mime: msg.mime || '', size: msg.size == null ? null : msg.size, sources: ['page'] });
      }
      if (!c) return sendResponse({ ok: false, error: 'Alamat video tidak valid.' });
      if (c.kind === 'manifest') return sendResponse({ ok: false, error: 'Aliran HLS/DASH belum dapat diunduh sebagai satu berkas.' });
      let referer = '';
      try { referer = (await chrome.tabs.get(tabId)).url || ''; } catch (e) { /* tab gone */ }
      try {
        await handOver('media.send_to_app', {
          url: c.url,
          mime_type: c.mime,
          file_name: UK.fileNameFromUrl(c.url),
          total_bytes: c.size,
          referer,
          user_agent: navigator.userAgent,
          cookies: await cookiesFor(c.url),
        });
        sendResponse({ ok: true });
      } catch (e) {
        sendResponse({ ok: false, error: e.code === 'ERR_NATIVE_HOST_NOT_FOUND' ? 'UnduhKilat tidak aktif. Jalankan aplikasi portabel.' : e.message || String(e.code) });
      }
    })();
    return true; // async response
  } else if (msg.type === 'popup-state') {
    sendResponse({ connected, enabled: settings.enabled, tracked: [...tracked.entries()].map(([id, t]) => ({ id, ...t })) });
  } else if (msg.type === 'set-enabled') {
    settings.enabled = !!msg.enabled;
    chrome.storage.local.set({ enabled: settings.enabled });
    sendResponse({ ok: true });
  }
  return false;
});

// ---- wiring ---------------------------------------------------------------------------------------------

chrome.downloads.onCreated.addListener(onDownloadCreated);
chrome.webRequest.onHeadersReceived.addListener(onHeaders, { urls: ['http://*/*', 'https://*/*'] }, ['responseHeaders']);
chrome.tabs.onRemoved.addListener((id) => tabCandidates.delete(id));
chrome.tabs.onUpdated.addListener((id, info) => {
  if (info.status === 'loading' && info.url) tabCandidates.delete(id); // new page: fresh candidate set
});

// Heartbeat: also reconnects after the worker slept and refreshes status (service-worker dormancy).
chrome.alarms.create('unduhkilat-heartbeat', { periodInMinutes: 0.4 });
chrome.alarms.onAlarm.addListener(async (a) => {
  if (a.name !== 'unduhkilat-heartbeat') return;
  try { applyPing(await call('ping', {}, 5000)); setConnected(true); await requery(); } catch (e) { setConnected(false); }
});

async function boot() {
  await loadState();
  updateBadge();
  call('ping', {}, 5000).then((d) => { applyPing(d); setConnected(true); requery(); }).catch(() => setConnected(false));
}
chrome.runtime.onStartup.addListener(boot);
chrome.runtime.onInstalled.addListener(boot);
boot();
