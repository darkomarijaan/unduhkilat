const test = require('node:test');
const assert = require('node:assert/strict');
const UK = require('../shared.js');

test('normalizeUrl drops fragments and range-only params, keeps real params', () => {
  assert.equal(UK.normalizeUrl('https://a.test/v.mp4?id=7&range=0-999&rn=3#t=10'), 'https://a.test/v.mp4?id=7');
  assert.equal(UK.normalizeUrl('https://a.test/v.mp4?id=7'), 'https://a.test/v.mp4?id=7');
});

test('normalizeUrl rejects non-http(s) schemes', () => {
  for (const u of ['blob:https://a.test/x', 'data:video/mp4;base64,AA', 'file:///C:/x', 'javascript:1', 'not a url']) {
    assert.equal(UK.normalizeUrl(u), null, u);
  }
});

test('classify separates downloadable files from HLS/DASH manifests', () => {
  assert.deepEqual(UK.classify('https://a/x.m3u8?t=1', ''), { kind: 'manifest', label: 'HLS' });
  assert.deepEqual(UK.classify('https://a/x', 'application/vnd.apple.mpegurl'), { kind: 'manifest', label: 'HLS' });
  assert.deepEqual(UK.classify('https://a/x.mpd', ''), { kind: 'manifest', label: 'DASH' });
  assert.deepEqual(UK.classify('https://a/x', 'application/dash+xml'), { kind: 'manifest', label: 'DASH' });
  assert.deepEqual(UK.classify('https://a/clip.mp4', ''), { kind: 'file', label: 'MP4' });
  assert.deepEqual(UK.classify('https://a/s', 'audio/mp4'), { kind: 'file', label: 'MP4' });
  assert.deepEqual(UK.classify('https://a/s', 'video/webm; codecs=vp9'), { kind: 'file', label: 'WEBM' });
  assert.deepEqual(UK.classify('https://a/s', ''), { kind: 'file', label: 'Media' });
});

test('isMediaResponse recognises network media', () => {
  assert.ok(UK.isMediaResponse('media', '', 'https://a/x'));
  assert.ok(UK.isMediaResponse('xmlhttprequest', 'video/mp4', 'https://a/x'));
  assert.ok(UK.isMediaResponse('xmlhttprequest', '', 'https://a/list.m3u8?x=1'));
  assert.ok(!UK.isMediaResponse('script', 'text/javascript', 'https://a/app.js'));
});

test('sizeFromHeaders prefers the Content-Range total and ignores partial lengths', () => {
  assert.equal(UK.sizeFromHeaders({ 'content-range': 'bytes 0-1/12345', 'content-length': '2' }), 12345);
  assert.equal(UK.sizeFromHeaders({ 'content-length': '999' }), 999);
  assert.equal(UK.sizeFromHeaders({ 'content-range': 'bytes 0-1/*', 'content-length': '2' }), null);
  assert.equal(UK.sizeFromHeaders({}), null);
});

test('mergeCandidate unions the two detection sources and dedups by resolved URL (FR-042)', () => {
  const m = new Map();
  UK.mergeCandidate(m, { url: 'https://a.test/v.mp4?range=0-10', mime: 'video/mp4', size: 5000, sources: ['network'] });
  UK.mergeCandidate(m, { url: 'https://a.test/v.mp4', mime: '', size: null, sources: ['page'] });
  UK.mergeCandidate(m, { url: 'https://a.test/only-page.webm', sources: ['page'] });
  UK.mergeCandidate(m, { url: 'https://a.test/only-net.mp4', sources: ['network'], size: 1 });
  assert.equal(m.size, 3, 'one entry per resolved URL');
  const both = m.get('https://a.test/v.mp4');
  assert.deepEqual(both.sources.sort(), ['network', 'page']);
  assert.equal(both.size, 5000, 'size discovered by one source is kept');
  assert.ok(m.has('https://a.test/only-page.webm'), 'page-only candidate still appears');
  assert.ok(m.has('https://a.test/only-net.mp4'), 'network-only candidate still appears');
});

test('mergeCandidate ignores unsupported URLs', () => {
  const m = new Map();
  assert.equal(UK.mergeCandidate(m, { url: 'blob:https://a/1', sources: ['page'] }), null);
  assert.equal(m.size, 0);
});

test('cookieHeader and file names', () => {
  assert.equal(UK.cookieHeader([{ name: 'a', value: '1' }, { name: 'b', value: '2' }]), 'a=1; b=2');
  assert.equal(UK.cookieHeader([]), '');
  assert.equal(UK.baseName('C:\\Users\\x\\Downloads\\f.zip'), 'f.zip');
  assert.equal(UK.baseName(''), '');
  assert.equal(UK.fileNameFromUrl('https://a.test/dir/My%20File.iso?x=1'), 'My File.iso');
});

test('isTinyMedia hides UI sounds but keeps playlists and unknown sizes', () => {
  assert.ok(UK.isTinyMedia('audio/mpeg', 'https://a/x.mp3', 6800));
  assert.ok(!UK.isTinyMedia('audio/mpeg', 'https://a/x.mp3', 5 * 1024 * 1024));
  assert.ok(!UK.isTinyMedia('audio/mpeg', 'https://a/x.mp3', null), 'unknown size is not filtered');
  assert.ok(!UK.isTinyMedia('application/vnd.apple.mpegurl', 'https://a/x', 900), 'playlists are small by nature');
  assert.ok(!UK.isTinyMedia('', 'https://a/stream.m3u8?x=1', 900));
});

test('isFreshDownload accepts only downloads that just started', () => {
  const now = Date.parse('2026-11-08T07:00:00Z');
  const at = (ms) => new Date(now + ms).toISOString();
  assert.ok(UK.isFreshDownload({ state: 'in_progress', startTime: at(-2000) }, now));
  assert.ok(UK.isFreshDownload({ state: 'in_progress', startTime: at(1000) }, now), 'small clock skew');
  assert.ok(UK.isFreshDownload({ state: 'in_progress' }, now), 'no start time: trust the state');
  assert.ok(!UK.isFreshDownload({ state: 'complete', startTime: at(-1000) }, now));
  assert.ok(!UK.isFreshDownload({ state: 'interrupted', startTime: at(-1000) }, now));
  assert.ok(!UK.isFreshDownload({ state: 'in_progress', startTime: at(-10 * 60 * 1000) }, now), 'old item resumed at startup');
  assert.ok(!UK.isFreshDownload({ state: 'in_progress', startTime: at(10 * 60 * 1000) }, now));
});

test('parseSkipList accepts common spellings and the app array', () => {
  assert.deepEqual(UK.parseSkipList(' PDF, *.jpg; .zip  mp4,,pdf '), ['pdf', 'jpg', 'zip', 'mp4']);
  assert.deepEqual(UK.parseSkipList(['pdf', 'JPG']), ['pdf', 'jpg']);
  assert.deepEqual(UK.parseSkipList(''), []);
  assert.deepEqual(UK.parseSkipList(undefined), []);
});

test('shouldSkipCapture matches by file name, URL path, or MIME', () => {
  const l = ['pdf'];
  assert.ok(UK.shouldSkipCapture(l, 'Laporan.PDF', 'https://a.test/x', ''));
  assert.ok(UK.shouldSkipCapture(l, '', 'https://a.test/files/report.pdf?dl=1', ''));
  assert.ok(UK.shouldSkipCapture(l, '', 'https://www.tokopedia.com/digital/invoice?id=3240614950', 'application/pdf'));
  assert.ok(UK.shouldSkipCapture(l, 'C:\\Users\\a\\Downloads\\x.pdf', 'https://a.test/get', ''));
  assert.ok(UK.shouldSkipCapture(['jpg'], '', 'https://a.test/img', 'image/jpeg'));
});

test('shouldSkipCapture keeps capturing other types and with an empty list', () => {
  assert.ok(!UK.shouldSkipCapture(['pdf'], 'setup.exe', 'https://a.test/setup.exe', 'application/x-msdownload'));
  assert.ok(!UK.shouldSkipCapture([], 'a.pdf', 'https://a.test/a.pdf', 'application/pdf'));
  assert.ok(!UK.shouldSkipCapture(['pdf'], '', 'https://a.test/pdf', ''));
  assert.ok(!UK.shouldSkipCapture(['pdf'], '.pdf', 'https://a.test/x', ''));
});
