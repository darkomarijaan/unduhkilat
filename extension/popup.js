const LABEL = { queued: 'Antre', running: 'Mengunduh', paused: 'Terjeda', completed: 'Selesai', failed: 'Gagal', cancelled: 'Dibatalkan' };

function pct(t) {
  return t.total > 0 ? Math.min(100, Math.floor((t.downloaded / t.total) * 100)) + '%' : '';
}

async function render() {
  const s = await chrome.runtime.sendMessage({ type: 'popup-state' });
  const state = document.getElementById('state');
  state.textContent = s.connected ? 'Terhubung' : 'Tidak aktif';
  state.className = s.connected ? 'ok' : 'bad';
  document.getElementById('enabled').checked = s.enabled;
  const list = document.getElementById('list');
  list.textContent = '';
  const items = (s.tracked || []).slice().reverse().slice(0, 20);
  document.getElementById('empty').hidden = items.length > 0;
  for (const t of items) {
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = t.fileName || t.id;
    const st = document.createElement('span');
    st.className = 'st';
    st.textContent = (LABEL[t.state] || t.state || '') + ' ' + pct(t); // text, never colour alone
    li.append(name, st);
    list.appendChild(li);
  }
}

document.getElementById('enabled').addEventListener('change', (e) => {
  chrome.runtime.sendMessage({ type: 'set-enabled', enabled: e.target.checked });
});

render();
setInterval(render, 1500);
