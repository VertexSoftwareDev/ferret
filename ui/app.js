/*
 * Ferret UI logic.
 *
 * Two things here are worth knowing before reading the rest.
 *
 * 1. The result list is virtualised. A query can match a million files, and
 *    building a million DOM rows would hang the window, so only the rows in
 *    view exist. A spacer div gives the scrollbar its true height and the row
 *    container is translated to the right offset.
 *
 * 2. Rows are fetched in windows from the backend rather than shipped in one
 *    response. The backend keeps the hit list from the last query, so paging is
 *    a slice, not a new search.
 */

'use strict';

const ROW_HEIGHT = 28;
/** Extra rows fetched above and below the viewport, to hide scroll latency. */
const OVERSCAN = 40;
/** How many rows one backend request covers. */
const WINDOW_SIZE = 400;
/** Typing settles for this long before a search runs. */
const DEBOUNCE_MS = 90;

const el = {
  query: document.getElementById('query'),
  drive: document.getElementById('drive'),
  rescan: document.getElementById('rescan'),
  theme: document.getElementById('theme'),
  viewport: document.getElementById('viewport'),
  sizer: document.getElementById('sizer'),
  rows: document.getElementById('rows'),
  count: document.getElementById('count'),
  volumeInfo: document.getElementById('volume-info'),
  notice: document.getElementById('notice'),
  overlay: document.getElementById('overlay'),
  overlayTitle: document.getElementById('overlay-title'),
  overlayDetail: document.getElementById('overlay-detail'),
  overlayAction: document.getElementById('overlay-action'),
  spinner: document.getElementById('spinner'),
  menu: document.getElementById('menu'),
  filters: {
    files: document.getElementById('f-files'),
    dirs: document.getElementById('f-dirs'),
    hidden: document.getElementById('f-hidden'),
    size: document.getElementById('f-size'),
  },
};

const state = {
  letter: '',
  total: 0,
  sortBy: '',
  descending: false,
  selected: -1,
  /** Rows cached from the backend: covers [cacheStart, cacheStart+rows.length). */
  cacheStart: 0,
  cacheRows: [],
  pendingWindow: null,
  searchToken: 0,
  needle: '',
  scanning: false,
};

/* ------------------------------------------------------------------ *
 * Backend bridge
 * ------------------------------------------------------------------ */

const tauri = window.__TAURI__;
const useMock = !tauri && new URLSearchParams(location.search).has('mock');

/** Call a backend command, or the mock stand-in when previewing in a browser. */
async function call(command, args) {
  if (tauri) return tauri.core.invoke(command, args);
  if (useMock) return mockCall(command, args);
  throw new Error('Ferret arka ucu bulunamadı.');
}

function listen(event, handler) {
  if (tauri) tauri.event.listen(event, (e) => handler(e.payload));
}

/**
 * Set the window title.
 *
 * `document.title` alone does not reach the native title bar in a Tauri window,
 * so the window API is asked directly and the document title kept in step for
 * the browser preview.
 */
function setWindowTitle(title) {
  document.title = title;
  if (tauri?.window?.getCurrentWindow) {
    tauri.window.getCurrentWindow().setTitle(title).catch(() => {});
  }
}

/* ------------------------------------------------------------------ *
 * Rendering
 * ------------------------------------------------------------------ */

const FILE_ICON =
  '<svg class="icon" viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>';
const DIR_ICON =
  '<svg class="icon dir" viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';

function escapeHtml(text) {
  return text.replace(
    /[&<>"']/g,
    (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c],
  );
}

/** Wrap the matched part of a name so the eye lands on it immediately. */
function highlight(name, needle) {
  const safe = escapeHtml(name);
  if (!needle) return safe;
  const at = name.toLowerCase().indexOf(needle);
  if (at < 0) return safe;
  const end = at + needle.length;
  return (
    escapeHtml(name.slice(0, at)) +
    '<mark>' +
    escapeHtml(name.slice(at, end)) +
    '</mark>' +
    escapeHtml(name.slice(end))
  );
}

function rowHtml(row, index) {
  const selected = index === state.selected ? ' selected' : '';
  return (
    `<div class="row${selected}" data-index="${index}" role="row">` +
    `<div class="cell name">${row.isDir ? DIR_ICON : FILE_ICON}<span>${highlight(row.name, state.needle)}</span></div>` +
    `<div class="cell path" title="${escapeHtml(row.folder)}">${escapeHtml(row.folder)}</div>` +
    `<div class="cell kind">${escapeHtml(row.kind)}</div>` +
    `<div class="cell size">${escapeHtml(row.size)}</div>` +
    `<div class="cell date">${escapeHtml(row.modified)}</div>` +
    '</div>'
  );
}

/** Row objects arrive snake_cased from serde; normalise once, here. */
function normalise(row) {
  return {
    name: row.name,
    path: row.path,
    folder: row.folder,
    kind: row.kind,
    size: row.size,
    modified: row.modified,
    isDir: row.is_dir,
  };
}

function rowAt(index) {
  const offset = index - state.cacheStart;
  if (offset < 0 || offset >= state.cacheRows.length) return null;
  return state.cacheRows[offset];
}

function render() {
  const viewportHeight = el.viewport.clientHeight;
  const scrollTop = el.viewport.scrollTop;

  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - 4);
  const visible = Math.ceil(viewportHeight / ROW_HEIGHT) + 8;
  const last = Math.min(state.total, first + visible);

  ensureWindow(first, last);

  let html = '';
  for (let i = first; i < last; i++) {
    const row = rowAt(i);
    html += row
      ? rowHtml(row, i)
      : `<div class="row" data-index="${i}" role="row"><div class="cell name">…</div><div class="cell"></div><div class="cell"></div><div class="cell"></div><div class="cell"></div></div>`;
  }

  el.rows.style.transform = `translateY(${first * ROW_HEIGHT}px)`;
  el.rows.innerHTML = html;
}

/** Make sure [first,last) is covered by the cache, fetching if it is not. */
function ensureWindow(first, last) {
  const covered =
    first >= state.cacheStart && last <= state.cacheStart + state.cacheRows.length;
  if (covered || state.total === 0) return;

  const start = Math.max(0, first - OVERSCAN);
  if (state.pendingWindow === start) return;
  state.pendingWindow = start;

  const token = state.searchToken;
  call('page', { offset: start, limit: WINDOW_SIZE })
    .then((rows) => {
      // A newer query landed while this was in flight.
      if (token !== state.searchToken) return;
      state.cacheStart = start;
      state.cacheRows = rows.map(normalise);
      state.pendingWindow = null;
      render();
    })
    .catch((err) => {
      state.pendingWindow = null;
      showNotice(String(err));
    });
}

/* ------------------------------------------------------------------ *
 * Searching
 * ------------------------------------------------------------------ */

let debounceTimer = null;

function scheduleSearch() {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(runSearch, DEBOUNCE_MS);
}

async function runSearch() {
  if (!state.letter || state.scanning) return;

  const text = el.query.value;
  const token = ++state.searchToken;

  // The needle for highlighting is the last path segment, matching what the
  // backend actually compares against the file name.
  const lastSegment = text.split(/[\\/]/).pop();
  state.needle = lastSegment.trim().toLowerCase();

  const args = {
    letter: state.letter,
    text,
    sortBy: state.sortBy,
    descending: state.descending,
    filesOnly: el.filters.files.checked,
    dirsOnly: el.filters.dirs.checked,
    skipHidden: el.filters.hidden.checked,
    minSize: el.filters.size.value ? Number(el.filters.size.value) : null,
    limit: WINDOW_SIZE,
  };

  try {
    const result = await call('search', { args });
    if (token !== state.searchToken) return;

    state.total = result.total;
    state.cacheStart = 0;
    state.cacheRows = result.rows.map(normalise);
    state.pendingWindow = null;
    state.selected = -1;

    el.sizer.style.height = `${state.total * ROW_HEIGHT}px`;
    el.viewport.scrollTop = 0;

    const totalText = result.total.toLocaleString('tr-TR');
    el.count.textContent = `${totalText} sonuç  ·  ${result.took_ms.toFixed(1)} ms`;
    // The window title carries the count too, so the taskbar says something
    // useful when Ferret is minimised.
    setWindowTitle(el.query.value ? `Ferret — ${totalText} sonuç` : titleForVolume());

    if (result.sort_skipped) {
      showNotice('Sonuç kümesi çok büyük olduğu için sıralama uygulanmadı.');
    } else {
      hideNotice();
    }

    render();
  } catch (err) {
    showNotice(String(err));
  }
}

/* ------------------------------------------------------------------ *
 * Volumes and scanning
 * ------------------------------------------------------------------ */

async function loadVolumes() {
  const volumes = await call('list_volumes');
  el.drive.innerHTML = volumes
    .map((v) => `<option value="${v.letter}">${v.letter}:</option>`)
    .join('');

  const indexed = volumes.find((v) => v.indexed);
  const preferred = indexed || volumes.find((v) => v.letter === 'C') || volumes[0];
  if (!preferred) {
    showOverlay('NTFS sürücüsü bulunamadı', 'Ferret yalnızca NTFS birimlerini okuyabilir.');
    return;
  }

  el.drive.value = preferred.letter;
  state.letter = preferred.letter;

  if (preferred.indexed) {
    setVolumeInfo(preferred);
    hideOverlay();
    await runSearch();
  } else {
    await scan(preferred.letter);
  }
}

async function scan(letter) {
  state.scanning = true;
  el.rescan.classList.add('busy');
  showOverlay(
    `${letter}: taranıyor`,
    'Ana dosya tablosu okunuyor. Bu, diskin tamamı için birkaç saniye sürer.',
  );

  try {
    const info = await call('scan_volume', { letter });
    setVolumeInfo(info);
    hideOverlay();
    state.scanning = false;
    await runSearch();
  } catch (err) {
    state.scanning = false;

    // Reading a raw volume needs administrator rights. Rather than demand them
    // before the window even opens, Ferret asks only once it actually needs
    // them — and offers to restart itself.
    const elevated = await call('is_elevated').catch(() => true);
    if (!elevated) {
      showOverlay(
        'Yönetici izni gerekiyor',
        'Ferret, diski doğrudan okuyarak indeksliyor; Windows bunun için yönetici izni istiyor. ' +
          'Diske yalnızca okuma yapılır, hiçbir şey yazılmaz.',
        'Yönetici olarak yeniden başlat',
        () => call('restart_elevated').catch((e) => showNotice(String(e))),
      );
    } else {
      showOverlay('Taranamadı', String(err), 'Tekrar dene', () => scan(letter));
    }
  } finally {
    el.rescan.classList.remove('busy');
  }
}

/** What the window title says when no query is active. */
let volumeTitle = 'Ferret';
/** The indexed volume, kept so the journal can refresh its counts in place. */
let volume = null;
let liveCount = 0;
let liveDirs = 0;

function titleForVolume() {
  return volumeTitle;
}

function setVolumeInfo(info) {
  volume = info;
  liveCount = info.files;
  liveDirs = info.dirs;
  refreshVolumeLine();
}

function refreshVolumeLine() {
  if (!volume) return;
  const files = liveCount.toLocaleString('tr-TR');
  const dirs = liveDirs.toLocaleString('tr-TR');

  el.volumeInfo.textContent =
    `${volume.letter}:  ${files} dosya · ${dirs} klasör · ` +
    `${volume.scan_seconds.toFixed(1)} sn'de tarandı · ${volume.memory}  ·  canlı`;

  volumeTitle = `Ferret — ${volume.letter}: ${files} dosya`;
  if (!el.query.value) setWindowTitle(volumeTitle);
}

/* ------------------------------------------------------------------ *
 * Overlay and notices
 * ------------------------------------------------------------------ */

function showOverlay(title, detail, actionLabel, action) {
  el.overlayTitle.textContent = title;
  el.overlayDetail.textContent = detail || '';
  el.spinner.hidden = Boolean(actionLabel);

  if (actionLabel) {
    el.overlayAction.textContent = actionLabel;
    el.overlayAction.hidden = false;
    el.overlayAction.onclick = action;
  } else {
    el.overlayAction.hidden = true;
    el.overlayAction.onclick = null;
  }
  el.overlay.hidden = false;
}

function hideOverlay() {
  el.overlay.hidden = true;
}

function showNotice(message) {
  el.notice.textContent = message;
  el.notice.hidden = false;
}

function hideNotice() {
  el.notice.hidden = true;
}

/* ------------------------------------------------------------------ *
 * Selection and actions
 * ------------------------------------------------------------------ */

function select(index) {
  if (index < 0 || index >= state.total) return;
  state.selected = index;

  // Keep the selection inside the viewport when moving by keyboard.
  const top = index * ROW_HEIGHT;
  const bottom = top + ROW_HEIGHT;
  if (top < el.viewport.scrollTop) {
    el.viewport.scrollTop = top;
  } else if (bottom > el.viewport.scrollTop + el.viewport.clientHeight) {
    el.viewport.scrollTop = bottom - el.viewport.clientHeight;
  }
  render();
}

function selectedRow() {
  return state.selected >= 0 ? rowAt(state.selected) : null;
}

async function activate(row) {
  if (!row) return;
  try {
    await call('open_path', { path: row.path });
  } catch (err) {
    showNotice(String(err));
  }
}

async function reveal(row) {
  if (!row) return;
  try {
    await call('reveal_path', { path: row.path });
  } catch (err) {
    showNotice(String(err));
  }
}

async function copyPath(row) {
  if (!row) return;
  try {
    await navigator.clipboard.writeText(row.path);
  } catch {
    // Clipboard can be denied; fall back to a selectable prompt-free notice.
    showNotice('Pano kullanılamadı: ' + row.path);
  }
}

/* ------------------------------------------------------------------ *
 * Context menu
 * ------------------------------------------------------------------ */

function openMenu(x, y) {
  el.menu.hidden = false;
  const { offsetWidth: w, offsetHeight: h } = el.menu;
  el.menu.style.left = `${Math.min(x, window.innerWidth - w - 6)}px`;
  el.menu.style.top = `${Math.min(y, window.innerHeight - h - 6)}px`;
}

function closeMenu() {
  el.menu.hidden = true;
}

/* ------------------------------------------------------------------ *
 * Theme
 * ------------------------------------------------------------------ */

const SUN =
  '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>';
const MOON = '<svg viewBox="0 0 24 24"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/></svg>';

function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  el.theme.innerHTML = theme === 'light' ? MOON : SUN;
  try {
    localStorage.setItem('ferret-theme', theme);
  } catch {
    /* storage can be unavailable; the theme still applies for this session */
  }
}

function initTheme() {
  let stored = null;
  try {
    stored = localStorage.getItem('ferret-theme');
  } catch {
    /* ignore */
  }
  const preferred =
    stored || (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');
  applyTheme(preferred);
}

/* ------------------------------------------------------------------ *
 * Events
 * ------------------------------------------------------------------ */

el.query.addEventListener('input', scheduleSearch);

el.drive.addEventListener('change', async () => {
  state.letter = el.drive.value;
  const volumes = await call('list_volumes');
  const info = volumes.find((v) => v.letter === state.letter);
  if (info && info.indexed) {
    setVolumeInfo(info);
    runSearch();
  } else {
    scan(state.letter);
  }
});

el.rescan.addEventListener('click', () => {
  if (!state.scanning && state.letter) scan(state.letter);
});

el.theme.addEventListener('click', () => {
  const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
  applyTheme(next);
});

for (const input of Object.values(el.filters)) {
  input.addEventListener('change', () => {
    // Files-only and dirs-only are mutually exclusive; ticking one clears the
    // other rather than silently returning nothing.
    if (input === el.filters.files && input.checked) el.filters.dirs.checked = false;
    if (input === el.filters.dirs && input.checked) el.filters.files.checked = false;
    runSearch();
  });
}

document.querySelectorAll('.th[data-sort]').forEach((th) => {
  th.addEventListener('click', () => {
    const column = th.dataset.sort;
    if (state.sortBy === column) {
      state.descending = !state.descending;
    } else {
      state.sortBy = column;
      state.descending = false;
    }
    document.querySelectorAll('.th').forEach((other) => delete other.dataset.dir);
    th.dataset.dir = state.descending ? 'desc' : 'asc';
    runSearch();
  });
});

el.viewport.addEventListener('scroll', render, { passive: true });

el.viewport.addEventListener('mousedown', (event) => {
  const row = event.target.closest('.row');
  if (row) select(Number(row.dataset.index));
});

el.viewport.addEventListener('dblclick', (event) => {
  const row = event.target.closest('.row');
  if (row) activate(rowAt(Number(row.dataset.index)));
});

el.viewport.addEventListener('contextmenu', (event) => {
  const row = event.target.closest('.row');
  if (!row) return;
  event.preventDefault();
  select(Number(row.dataset.index));
  openMenu(event.clientX, event.clientY);
});

el.menu.addEventListener('click', (event) => {
  const action = event.target.dataset.action;
  const row = selectedRow();
  closeMenu();
  if (action === 'open') activate(row);
  if (action === 'reveal') reveal(row);
  if (action === 'copy') copyPath(row);
});

window.addEventListener('mousedown', (event) => {
  if (!el.menu.contains(event.target)) closeMenu();
});

window.addEventListener('resize', render);

window.addEventListener('keydown', (event) => {
  if (event.key === 'f' && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    el.query.select();
    el.query.focus();
    return;
  }
  if (event.key === 'F5') {
    event.preventDefault();
    if (!state.scanning && state.letter) scan(state.letter);
    return;
  }
  if (event.key === 'Escape') {
    if (!el.menu.hidden) return closeMenu();
    if (el.query.value) {
      el.query.value = '';
      runSearch();
    }
    return;
  }

  const typingInSearch = document.activeElement === el.query;

  switch (event.key) {
    case 'ArrowDown':
      event.preventDefault();
      select(state.selected + 1);
      break;
    case 'ArrowUp':
      event.preventDefault();
      select(Math.max(0, state.selected - 1));
      break;
    case 'PageDown':
      event.preventDefault();
      select(state.selected + Math.floor(el.viewport.clientHeight / ROW_HEIGHT));
      break;
    case 'PageUp':
      event.preventDefault();
      select(Math.max(0, state.selected - Math.floor(el.viewport.clientHeight / ROW_HEIGHT)));
      break;
    case 'Home':
      if (!typingInSearch) {
        event.preventDefault();
        select(0);
      }
      break;
    case 'End':
      if (!typingInSearch) {
        event.preventDefault();
        select(state.total - 1);
      }
      break;
    case 'Enter':
      if (state.selected >= 0) {
        event.preventDefault();
        activate(selectedRow());
      }
      break;
    case 'c':
      if (event.ctrlKey && state.selected >= 0 && !typingInSearch) {
        event.preventDefault();
        copyPath(selectedRow());
      }
      break;
    default:
      break;
  }
});

listen('scan-progress', (message) => {
  if (state.scanning) el.overlayDetail.textContent = message;
});

/**
 * The change journal folded new activity into the index.
 *
 * The counts are refreshed always, but the result list is only re-run when the
 * user is sitting at the top of it. Re-sorting the ground under someone who is
 * scrolling through results is worse than showing them a row that is a few
 * seconds out of date.
 */
listen('index-updated', (update) => {
  if (!update || update.letter !== state.letter) return;

  liveCount = update.files;
  liveDirs = update.dirs;
  refreshVolumeLine();

  if (!state.scanning && el.viewport.scrollTop < ROW_HEIGHT * 2) {
    scheduleSearch();
  }
});

/** The journal wrapped or was reset: the index can no longer be patched. */
listen('index-stale', () => {
  showNotice('Disk çok değişti; güncel sonuçlar için F5 ile yeniden tara.');
});

/* ------------------------------------------------------------------ *
 * Mock backend — only for previewing the UI in a plain browser with ?mock=1.
 * ------------------------------------------------------------------ */

function mockCall(command, args) {
  if (!mockCall.data) {
    const words = ['rapor', 'notlar', 'foto', 'setup', 'belge', 'yedek', 'kurulum', 'video'];
    const exts = ['txt', 'pdf', 'docx', 'jpg', 'exe', 'zip', 'mp4'];
    mockCall.data = Array.from({ length: 120000 }, (_, i) => {
      const isDir = i % 11 === 0;
      const name = isDir
        ? `${words[i % words.length]}_${i}`
        : `${words[i % words.length]}_${i}.${exts[i % exts.length]}`;
      return {
        name,
        path: `C:\\Users\\pc\\Ornek\\${i % 50}\\${name}`,
        folder: `C:\\Users\\pc\\Ornek\\${i % 50}`,
        kind: isDir ? 'Klasör' : exts[i % exts.length].toUpperCase(),
        size: isDir ? '' : `${((i % 900) + 1) / 10} MB`,
        size_bytes: (i % 900) * 1024,
        modified: `2026-0${(i % 9) + 1}-1${i % 9} 1${i % 9}:0${i % 6}`,
        is_dir: isDir,
      };
    });
  }
  const all = mockCall.data;

  if (command === 'list_volumes') {
    return Promise.resolve([
      { letter: 'C', indexed: true, files: 1537205, dirs: 292891, scan_seconds: 6.7, memory: '161 MB' },
    ]);
  }
  if (command === 'scan_volume') {
    return new Promise((r) =>
      setTimeout(
        () => r({ letter: 'C', indexed: true, files: 1537205, dirs: 292891, scan_seconds: 6.7, memory: '161 MB' }),
        900,
      ),
    );
  }
  if (command === 'search') {
    const needle = (args.args.text || '').toLowerCase();
    mockCall.hits = all.filter((r) => r.name.toLowerCase().includes(needle));
    return Promise.resolve({
      total: mockCall.hits.length,
      took_ms: 4.2,
      rows: mockCall.hits.slice(0, args.args.limit || 400),
      sort_skipped: false,
    });
  }
  if (command === 'page') {
    return Promise.resolve(mockCall.hits.slice(args.offset, args.offset + args.limit));
  }
  return Promise.resolve(null);
}

/* ------------------------------------------------------------------ *
 * Start
 * ------------------------------------------------------------------ */

initTheme();
el.query.focus();

loadVolumes().catch((err) => {
  showOverlay(
    'Başlatılamadı',
    String(err),
    'Tekrar dene',
    () => loadVolumes().catch(() => {}),
  );
});
