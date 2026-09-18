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
  lang: document.getElementById('lang'),
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
  progress: document.getElementById('progress'),
  progressFill: document.getElementById('progress-fill'),
  menu: document.getElementById('menu'),
  empty: document.getElementById('empty'),
  emptyTitle: document.getElementById('empty-title'),
  emptyHint: document.getElementById('empty-hint'),
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
  /** Drive chosen in a previous run, restored before the first search. */
  preferredDrive: '',
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
  throw new Error('no_backend');
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
 * Language
 * ------------------------------------------------------------------ */

let lang = initialLanguage();
/** The active string table. Read through this, never from a literal. */
let t = I18N[lang];

/** Format a number the way the chosen language writes numbers. */
function num(value) {
  return value.toLocaleString(t.locale);
}

/**
 * Turn a backend error into a sentence.
 *
 * Rust sends `code` or `code:detail`; anything unrecognised is shown as it
 * arrived, because an OS message is more useful than "something went wrong".
 */
function say(error) {
  const text = String(error && error.message ? error.message : error);
  const cut = text.indexOf(':');
  const code = cut < 0 ? text : text.slice(0, cut);
  const detail = cut < 0 ? '' : text.slice(cut + 1);

  const entry = t.errors[code];
  if (!entry) return text;
  return typeof entry === 'function' ? entry(detail) : entry;
}

function applyLanguage(code) {
  lang = I18N[code] ? code : 'en';
  t = I18N[lang];
  document.documentElement.lang = lang;
  el.lang.value = lang;
  rememberLanguage(lang);

  for (const node of document.querySelectorAll('[data-i18n]')) {
    node.textContent = t[node.dataset.i18n] ?? '';
  }
  for (const node of document.querySelectorAll('[data-i18n-placeholder]')) {
    node.placeholder = t[node.dataset.i18nPlaceholder] ?? '';
  }
  for (const node of document.querySelectorAll('[data-i18n-title]')) {
    node.title = t[node.dataset.i18nTitle] ?? '';
  }
  for (const node of document.querySelectorAll('[data-i18n-aria]')) {
    node.setAttribute('aria-label', t[node.dataset.i18nAria] ?? '');
  }

  // Anything already on screen was worded in the previous language.
  refreshVolumeLine();
  refreshCount();
  render();
}

/* ------------------------------------------------------------------ *
 * Remembered preferences
 * ------------------------------------------------------------------ */

const PREFS_KEY = 'ferret-prefs';

/**
 * Read and write the handful of choices worth surviving a restart.
 *
 * Deliberately not the query itself: reopening Ferret to yesterday's search
 * would be a surprise, and the box is where the eye goes first anyway.
 */
function loadPrefs() {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

function savePrefs() {
  try {
    localStorage.setItem(
      PREFS_KEY,
      JSON.stringify({
        drive: state.letter,
        sortBy: state.sortBy,
        descending: state.descending,
        filesOnly: el.filters.files.checked,
        dirsOnly: el.filters.dirs.checked,
        skipHidden: el.filters.hidden.checked,
        minSize: el.filters.size.value,
      }),
    );
  } catch {
    /* storage can be blocked; the choices still hold for this session */
  }
}

/** Put the saved choices back on screen, before the first search runs. */
function restorePrefs() {
  const prefs = loadPrefs();

  if (prefs.sortBy) {
    state.sortBy = prefs.sortBy;
    state.descending = Boolean(prefs.descending);
    const header = document.querySelector(`.th[data-sort="${CSS.escape(prefs.sortBy)}"]`);
    if (header) header.dataset.dir = state.descending ? 'desc' : 'asc';
  }
  if (typeof prefs.filesOnly === 'boolean') el.filters.files.checked = prefs.filesOnly;
  if (typeof prefs.dirsOnly === 'boolean') el.filters.dirs.checked = prefs.dirsOnly;
  if (typeof prefs.skipHidden === 'boolean') el.filters.hidden.checked = prefs.skipHidden;
  if (typeof prefs.minSize === 'string') el.filters.size.value = prefs.minSize;
  if (typeof prefs.drive === 'string') state.preferredDrive = prefs.drive;
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
    `<div class="cell kind">${escapeHtml(row.isDir ? t.folder : row.kind)}</div>` +
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
  renderEmptyState();
}

/**
 * Say why the list is blank.
 *
 * An empty table with no explanation reads as a broken app, and the two reasons
 * a query comes back with nothing — no such name, or the filters ruled it all
 * out — need different advice.
 */
function renderEmptyState() {
  const query = el.query.value.trim();
  const blank = state.total === 0 && query !== '' && !state.scanning;
  el.empty.hidden = !blank;
  if (!blank) return;

  // Only filters the user actually chose count. "Skip hidden" is on by
  // default, and blaming it for an empty result the moment someone mistypes a
  // name would send them looking in the wrong place.
  const filtering =
    el.filters.files.checked || el.filters.dirs.checked || el.filters.size.value !== '';

  el.emptyTitle.textContent = t.emptyTitle(query);
  el.emptyHint.textContent = filtering ? t.emptyFiltered : t.emptyHint;
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
      showNotice(say(err));
    });
}

/* ------------------------------------------------------------------ *
 * Searching
 * ------------------------------------------------------------------ */

let debounceTimer = null;
/** Milliseconds the last query took, kept so the status bar can be re-worded. */
let lastTiming = null;
/** Combined size of the last result set, already formatted by the backend. */
let lastSize = '';

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

    lastTiming = result.took_ms;
    lastSize = result.total_size || '';
    refreshCount();

    if (result.sort_skipped) {
      showNotice(t.sortSkipped);
    } else {
      hideNotice();
    }

    render();
  } catch (err) {
    showNotice(say(err));
  }
}

/** The status bar's left half, and the window title that mirrors it. */
function refreshCount() {
  if (lastTiming === null) return;
  const total = num(state.total);
  const parts = [t.results(total)];
  // The total size only says something once there is something to add up.
  if (state.total > 0 && lastSize) parts.push(t.totalSize(lastSize));
  parts.push(t.timing(lastTiming.toFixed(1)));
  el.count.textContent = parts.join('  ·  ');
  // The window title carries the count too, so the taskbar says something
  // useful when Ferret is minimised.
  setWindowTitle(el.query.value ? t.titleResults(total) : titleForVolume());
}

/* ------------------------------------------------------------------ *
 * Volumes and scanning
 * ------------------------------------------------------------------ */

async function loadVolumes() {
  const volumes = await call('list_volumes');
  el.drive.innerHTML = volumes
    .map((v) => `<option value="${v.letter}">${v.letter}:</option>`)
    .join('');

  const remembered = volumes.find((v) => v.letter === state.preferredDrive);
  const indexed = volumes.find((v) => v.indexed);
  const preferred = remembered || indexed || volumes.find((v) => v.letter === 'C') || volumes[0];
  if (!preferred) {
    showOverlay(t.noVolumeTitle, t.noVolumeDetail);
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
  showOverlay(t.scanning(letter), t.scanningDetail);
  setProgress(0);

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
        t.elevationTitle,
        t.elevationDetail,
        t.elevationAction,
        () => call('restart_elevated').catch((e) => showNotice(say(e))),
      );
    } else {
      showOverlay(t.scanFailed, say(err), t.retry, () => scan(letter));
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
  const files = num(liveCount);
  const dirs = num(liveDirs);

  el.volumeInfo.textContent = t.volumeLine(
    volume.letter,
    files,
    dirs,
    volume.scan_seconds.toFixed(1),
    volume.memory,
  );

  volumeTitle = t.titleVolume(volume.letter, files);
  if (!el.query.value) setWindowTitle(volumeTitle);
}

/* ------------------------------------------------------------------ *
 * Overlay and notices
 * ------------------------------------------------------------------ */

function showOverlay(title, detail, actionLabel, action) {
  el.overlayTitle.textContent = title;
  el.overlayDetail.textContent = detail || '';
  el.spinner.hidden = Boolean(actionLabel);
  // A bar belongs to a scan; an error screen gets a button instead.
  el.progress.hidden = true;

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
    showNotice(say(err));
  }
}

async function reveal(row) {
  if (!row) return;
  try {
    await call('reveal_path', { path: row.path });
  } catch (err) {
    showNotice(say(err));
  }
}

/**
 * Narrow the search to the folder a result sits in.
 *
 * Finding one file and then wanting its neighbours is the commonest follow-up
 * there is, and retyping the path by hand is the commonest annoyance.
 */
function searchInFolder(row) {
  if (!row) return;
  // A folder's own name scopes to itself; a file scopes to its parent.
  const folder = row.isDir ? row.path : row.folder;
  const drive = `${state.letter}:\\`;
  const relative = folder.startsWith(drive) ? folder.slice(drive.length) : folder;

  el.query.value = relative ? `${relative}\\` : '';
  el.query.focus();
  runSearch();
}

async function copyPath(row) {
  if (!row) return;
  try {
    await navigator.clipboard.writeText(row.path);
  } catch {
    // Clipboard can be denied; fall back to showing the path so it can still
    // be copied by hand.
    showNotice(t.clipboardFailed(row.path));
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
  savePrefs();
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

el.lang.addEventListener('change', () => applyLanguage(el.lang.value));

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
    savePrefs();
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
    savePrefs();
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
  if (action === 'here') searchInFolder(row);
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

/**
 * Show how far the scan has got.
 *
 * Seven seconds behind a spinner that says nothing is the app's worst moment;
 * a bar that visibly moves turns the same wait into something ordinary.
 */
function setProgress(percent) {
  el.progress.hidden = false;
  el.progressFill.style.width = `${Math.max(0, Math.min(100, percent))}%`;
}

listen('scan-progress', (progress) => {
  if (state.scanning && progress && progress.letter === state.letter) {
    setProgress(progress.percent);
  }
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
  showNotice(t.indexStale);
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
        kind: isDir ? '' : exts[i % exts.length].toUpperCase(),
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
      total_size: '1.4 GB',
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
applyLanguage(lang);
restorePrefs();
el.query.focus();

loadVolumes().catch((err) => {
  showOverlay(t.startFailed, say(err), t.retry, () => loadVolumes().catch(() => {}));
});
