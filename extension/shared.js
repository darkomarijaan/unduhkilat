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

  const api = { RANGE_PARAMS, isHttp, normalizeUrl, classify, isMediaResponse, isTinyMedia, headerMap, sizeFromHeaders, cookieHeader, baseName, fileNameFromUrl, mergeCandidate };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  root.UK = api;
})(typeof self !== 'undefined' ? self : globalThis);
