// Page side of the hybrid media detector (DEC-003): hooks <video>/<audio> elements and
// renders the floating panel. Network-sniffed candidates arrive from the service worker.
/* global UK */
(function () {
  'use strict';
  if (window.top !== window) return; // one panel per tab; frames report via their own hooks only
  if (window.__unduhkilatLoaded) return;
  window.__unduhkilatLoaded = true;

  let candidates = [];
  let connected = true;
  let open = false;
  let flash = '';

  // ---- DOM hook ----------------------------------------------------------------------------
  const seen = new Set();
  function scan() {
    const items = [];
    for (const el of document.querySelectorAll('video, audio')) {
      const urls = [el.currentSrc, el.src];
      for (const s of el.querySelectorAll('source')) urls.push(s.src);
      for (const u of urls) {
        if (u && UK.isHttp(u) && !seen.has(u)) {
          seen.add(u);
          items.push({ url: u, mime: el.tagName === 'AUDIO' ? 'audio/*' : '' });
        }
      }
    }
    if (items.length) chrome.runtime.sendMessage({ type: 'dom-media', items }).catch(() => {});
  }
  new MutationObserver(scan).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['src'] });
  document.addEventListener('loadedmetadata', scan, true);
  document.addEventListener('play', scan, true);
  scan();

  // ---- floating panel (Shadow DOM so page CSS cannot touch it) -----------------------------
  const host = document.createElement('div');
  host.style.cssText = 'all:initial;position:fixed;z-index:2147483647;top:12px;right:12px;display:none;';
  const root = host.attachShadow({ mode: 'closed' });
  root.innerHTML = `
    <style>
      :host { all: initial; }
      * { box-sizing: border-box; font-family: "Segoe UI", Inter, sans-serif; }
      .pill { display:flex; align-items:center; gap:6px; background:#00D2FF; color:#08131E; border:0; border-radius:2px; padding:6px 12px; font-size:13px; font-weight:600; cursor:pointer; }
      .pill:focus-visible, button:focus-visible { outline:2px solid #38BDF8; outline-offset:2px; }
      .panel { margin-top:6px; width:360px; max-height:300px; overflow:auto; background:#1A1E26; color:#E6EDF5; border:1px solid #2D3442; border-radius:4px; box-shadow:0 8px 24px rgba(0,0,0,.6); }
      .row { display:grid; grid-template-columns:54px 1fr auto; gap:8px; align-items:center; padding:8px 10px; border-bottom:1px solid #2D3442; font-size:12px; }
      .row:last-child { border-bottom:0; }
      .fmt { font-family:"Cascadia Mono", Consolas, monospace; font-weight:600; }
      .meta { color:#94A1B2; font-family:"Cascadia Mono", Consolas, monospace; font-size:11px; }
      .src { color:#5E6A7E; font-size:10px; }
      .warn { color:#F59E0B; font-size:11px; }
      .go { background:#222732; color:#E6EDF5; border:1px solid #2D3442; border-radius:2px; padding:5px 10px; font-size:12px; cursor:pointer; }
      .go:hover:not(:disabled) { background:#2D3442; }
      .go:disabled { color:#5E6A7E; cursor:not-allowed; }
      .msg { padding:8px 10px; font-size:12px; color:#94A1B2; }
      .msg.err { color:#EF4444; }
    </style>
    <button class="pill" id="pill" aria-expanded="false">
      <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true"><polygon points="8.1,0.6 2.8,7.8 6.4,7.8 5.3,13.4 11.2,5.6 7.6,5.6" fill="#08131E"/></svg>
      <span id="label">Unduh Video</span>
    </button>
    <div class="panel" id="panel" hidden></div>`;
  const $ = (id) => root.getElementById(id);
  (document.body || document.documentElement).appendChild(host);

  function fmtSize(n) {
    if (n == null) return 'Ukuran tidak diketahui';
    const u = ['B', 'KB', 'MB', 'GB']; let v = n, i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return (i ? v.toFixed(1) : v) + ' ' + u[i];
  }
  const SRC = { network: '[Jaringan]', page: '[Halaman]' };

  function render() {
    host.style.display = candidates.length ? 'block' : 'none'; // hidden when nothing was detected
    $('label').textContent = 'Unduh Video (' + candidates.length + ')';
    $('pill').setAttribute('aria-expanded', String(open));
    const panel = $('panel');
    panel.hidden = !open;
    if (!open) return;
    panel.textContent = '';
    if (flash) {
      const d = document.createElement('div');
      d.className = 'msg' + (flash.startsWith('!') ? ' err' : '');
      d.textContent = flash.replace(/^!/, '');
      panel.appendChild(d);
    }
    if (!connected) {
      const d = document.createElement('div');
      d.className = 'msg err';
      d.textContent = 'UnduhKilat tidak aktif. Jalankan aplikasi portabel.';
      panel.appendChild(d);
    }
    for (const c of candidates) {
      const row = document.createElement('div');
      row.className = 'row';
      const fmt = document.createElement('div'); fmt.className = 'fmt'; fmt.textContent = c.label;
      const info = document.createElement('div');
      const size = document.createElement('div'); size.className = 'meta'; size.textContent = fmtSize(c.size);
      const src = document.createElement('div'); src.className = 'src'; src.textContent = c.sources.map((s) => SRC[s] || s).join(' ');
      info.append(size, src);
      if (c.kind === 'manifest') {
        const w = document.createElement('div'); w.className = 'warn'; w.textContent = 'Aliran adaptif: belum dapat diunduh sebagai satu berkas';
        info.append(w);
      }
      const go = document.createElement('button');
      go.className = 'go'; go.textContent = 'Unduh Berkas'; go.disabled = c.kind === 'manifest';
      go.addEventListener('click', async () => {
        go.disabled = true;
        const r = await chrome.runtime.sendMessage({ type: 'send-media', url: c.url, mime: c.mime, size: c.size }).catch(() => ({ ok: false, error: 'Tidak dapat menghubungi ekstensi.' }));
        flash = r && r.ok ? 'Terkirim. Konfirmasi di jendela UnduhKilat untuk mulai mengunduh.' : '!' + ((r && r.error) || 'Gagal mengirim.');
        render();
      });
      row.append(fmt, info, go);
      panel.appendChild(row);
    }
  }

  $('pill').addEventListener('click', () => { open = !open; flash = ''; render(); });

  chrome.runtime.onMessage.addListener((m) => {
    if (m && m.type === 'candidates') { candidates = m.list || []; render(); }
  });
  chrome.runtime.sendMessage({ type: 'get-candidates' }).then((r) => {
    if (r) { candidates = r.list || []; connected = r.connected !== false; render(); }
  }).catch(() => {});
})();
