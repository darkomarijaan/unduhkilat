// Pure helpers shared by the service worker and the content script.
// No chrome.* access here so they can be unit-tested with Node.
(function (root) {
  'use strict';

  // Query parameters that only select a byte range of the SAME resource; they must not
  // make one video look like many candidates (FR-042 dedup by resolved URL).
  const RANGE_PARAMS = ['range', 'bytestart', 'byteend', 'rn', 'rbuf'];

  function isHttp(u) {
    return typeof u === 'string' && /^https?:\/\//i.test(u);
  }

  function normalizeUrl(u) {
    try {
      const x = new URL(u);
      if (x.protocol !== 'http:' && x.protocol !== 'https:') return null;
      x.hash = '';
      for (const p of RANGE_PARAMS) x.searchParams.delete(p);
      return x.toString();
    } catch (e) {
      return null;
    }
  }

  const EXT_LABELS = {
    mp4: 'MP4', m4v: 'MP4', webm: 'WEBM', mkv: 'MKV', mov: 'MOV', avi: 'AVI', flv: 'FLV',
    mp3: 'MP3', m4a: 'M4A', aac: 'AAC', ogg: 'OGG', opus: 'OPUS', wav: 'WAV', flac: 'FLAC',
  };

  /**
   * kind 'manifest' = HLS/DASH playlist (FR-044: listed, but not downloadable as one file).
   * kind 'file'     = a directly downloadable media resource.
   */
  function classify(url, mime) {
    let path = '';
    try { path = new URL(url).pathname.toLowerCase(); } catch (e) { /* keep '' */ }
    const m = (mime || '').toLowerCase().split(';')[0].trim();
    if (/mpegurl/.test(m) || /\.m3u8$/.test(path)) return { kind: 'manifest', label: 'HLS' };
    if (/dash\+xml/.test(m) || /\.mpd$/.test(path)) return { kind: 'manifest', label: 'DASH' };
    const ext = (path.match(/\.([a-z0-9]{2,5})$/) || [])[1];
    if (ext && EXT_LABELS[ext]) return { kind: 'file', label: EXT_LABELS[ext] };
    const sub = (m.match(/^(?:video|audio)\/([a-z0-9.+-]+)/) || [])[1];
    if (sub) return { kind: 'file', label: (EXT_LABELS[sub] || sub).toUpperCase() };
    return { kind: 'file', label: 'Media' };
  }

  function isMediaResponse(type, mime, url) {
    const m = (mime || '').toLowerCase();
    return type === 'media' || /^(video|audio)\//.test(m) || /mpegurl|dash\+xml/.test(m) || /\.(m3u8|mpd)(\?|#|$)/i.test(url || '');
  }

  /** Smaller than this is UI noise (click/notification sounds), not something to download. */
  const MIN_MEDIA_BYTES = 64 * 1024;

  /** A response whose known size is tiny and that is not a playlist/manifest. Unknown size is kept. */
  function isTinyMedia(mime, url, size) {
    if (size == null || size >= MIN_MEDIA_BYTES) return false;
    return !(/mpegurl|dash\+xml/i.test(mime || '') || /\.(m3u8|mpd)(\?|#|$)/i.test(url || ''));
  }

  function headerMap(list) {
    const out = {};
    for (const h of list || []) out[String(h.name).toLowerCase()] = h.value;
    return out;
  }

  /** Total size from Content-Range ("bytes 0-1/12345") else Content-Length; null if unknown. */
  function sizeFromHeaders(h) {
    const cr = h['content-range'];
    if (cr) {
      const t = String(cr).split('/').pop();
      const n = parseInt(t, 10);
      if (Number.isFinite(n) && n >= 0) return n;
    }
    const cl = parseInt(h['content-length'], 10);
    return Number.isFinite(cl) && cl > 0 && !cr ? cl : null;
  }

  function cookieHeader(cookies) {
    return (cookies || []).map((c) => c.name + '=' + c.value).join('; ');
  }

  function baseName(p) {
    if (!p) return '';
    return String(p).split(/[\\/]/).pop() || '';
  }

  function fileNameFromUrl(url) {
    try {
      const seg = new URL(url).pathname.split('/').pop() || '';
      return decodeURIComponent(seg);
    } catch (e) {
      return '';
    }
  }

  /**
   * Union of candidates from the two detection sources (DEC-003, FR-041/042).
   * A candidate seen by only one source still appears; sources accumulate on duplicates.
   */
  function mergeCandidate(map, c) {
    const key = normalizeUrl(c.url);
    if (!key) return null;
    const cls = classify(c.url, c.mime);
    const prev = map.get(key);
    if (prev) {
      for (const s of c.sources || []) if (!prev.sources.includes(s)) prev.sources.push(s);
      if (c.size != null && prev.size == null) prev.size = c.size;
      if (c.mime && !prev.mime) prev.mime = c.mime;
      return prev;
    }
    const entry = { key, url: c.url, mime: c.mime || '', size: c.size == null ? null : c.size, sources: [...(c.sources || [])], kind: cls.kind, label: cls.label };
    map.set(key, entry);
    return entry;
  }

  // How long after a download starts it still counts as "just started by the user".
  const FRESH_WINDOW_MS = 60 * 1000;

  /**
   * True only for a download the user just started. At browser startup Chrome loads its
   * download history and fires downloads.onCreated for every entry; those (finished,
   * interrupted, or old ones auto-resumed) must never be handed over or erased.
   */
  function isFreshDownload(item, nowMs) {
    if (!item || item.state !== 'in_progress') return false;
    if (!item.startTime) return true;
    const t = Date.parse(item.startTime);
    if (Number.isNaN(t)) return true;
    return Math.abs((nowMs == null ? Date.now() : nowMs) - t) <= FRESH_WINDOW_MS;
  }

  // ---- jenis berkas yang dibiarkan diunduh browser ----------------------------------------
  // Kembar dengan crates/unduhkilat-core/src/capture_filter.rs; ubah keduanya bersamaan.
  const SKIP_MIME_EXTS = {
    'application/pdf': ['pdf'], 'application/x-pdf': ['pdf'],
    'application/zip': ['zip'], 'application/x-zip-compressed': ['zip'],
    'application/vnd.rar': ['rar'], 'application/x-rar-compressed': ['rar'],
    'application/x-7z-compressed': ['7z'], 'application/gzip': ['gz'], 'application/x-gzip': ['gz'], 'application/x-tar': ['tar'],
    'application/x-msdownload': ['exe'], 'application/x-msdos-program': ['exe'], 'application/vnd.microsoft.portable-executable': ['exe'],
    'application/x-msi': ['msi'], 'application/x-ms-installer': ['msi'], 'application/x-iso9660-image': ['iso'],
    'application/vnd.android.package-archive': ['apk'], 'application/x-bittorrent': ['torrent'], 'application/epub+zip': ['epub'],
    'application/msword': ['doc'], 'application/vnd.openxmlformats-officedocument.wordprocessingml.document': ['docx'],
    'application/vnd.ms-excel': ['xls'], 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': ['xlsx'],
    'application/vnd.ms-powerpoint': ['ppt'], 'application/vnd.openxmlformats-officedocument.presentationml.presentation': ['pptx'],
    'text/csv': ['csv'], 'text/plain': ['txt'],
    'image/jpeg': ['jpg', 'jpeg'], 'image/png': ['png'], 'image/gif': ['gif'], 'image/webp': ['webp'], 'image/svg+xml': ['svg'],
    'video/mp4': ['mp4'], 'video/webm': ['webm'], 'video/x-matroska': ['mkv'], 'audio/mpeg': ['mp3'],
  };

  /** "PDF, *.jpg; .zip" -> ['pdf','jpg','zip']. Accepts an array too (as sent by the app). */
  function parseSkipList(v) {
    const parts = Array.isArray(v) ? v : String(v || '').split(/[,;\s]+/);
    const out = [];
    for (const raw of parts) {
      const e = String(raw).trim().replace(/^\*/, '').replace(/^\./, '').toLowerCase();
      if (e && /^[a-z0-9]+$/.test(e) && !out.includes(e)) out.push(e);
    }
    return out;
  }

  function extOf(name) {
    const base = String(name || '').split(/[\\/]/).pop();
    const m = base.match(/^(.+)\.([A-Za-z0-9]{1,10})$/);
    return m ? m[2].toLowerCase() : null;
  }

  /** True if this browser download is a type the user wants the browser to keep. */
  function shouldSkipCapture(list, fileName, url, mime) {
    if (!list || !list.length) return false;
    const exts = [];
    const fe = extOf(fileName);
    if (fe) exts.push(fe);
    try { const ue = extOf(new URL(url).pathname); if (ue) exts.push(ue); } catch (e) { /* no URL ext */ }
    const m = String(mime || '').split(';')[0].trim().toLowerCase();
    for (const e of SKIP_MIME_EXTS[m] || []) exts.push(e);
    return exts.some((e) => list.includes(e));
  }

  const api = { parseSkipList, shouldSkipCapture, RANGE_PARAMS, FRESH_WINDOW_MS, isFreshDownload, isHttp, normalizeUrl, classify, isMediaResponse, isTinyMedia, headerMap, sizeFromHeaders, cookieHeader, baseName, fileNameFromUrl, mergeCandidate };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  root.UK = api;
})(typeof self !== 'undefined' ? self : globalThis);
