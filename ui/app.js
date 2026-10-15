/* ============================================================================
   RustWinUtil front-end.

   The tabs are built from WinUtil's own config (apps, tweaks, features, AppX),
   handed over by the host at start-up, the way WinUtil builds its WPF tabs from
   the same JSON. Every action is sent to the host as a job; the host runs one at
   a time and reports back through window.onHost.
   ========================================================================== */

const BOOT = window.BOOT;
const post = (msg) => window.ipc && window.ipc.postMessage(JSON.stringify(msg));
const $ = (id) => document.getElementById(id);

function h(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
}

const state = {
  tab: 'install',
  query: '',
  category: 'All',
  selectedApps: new Set(),
  installedApps: new Set(),
  collapsed: new Set(),
  appliedTweaks: new Set(),
  /* Ticked tweak and feature checkboxes, kept here so a search can't drop them. */
  ticked: new Set(),
  /* The DNS provider for the next Run Tweaks; "Default" leaves DNS as it is. */
  dns: 'Default',
  toggles: {},
  combos: {},
  selectedAppx: new Set(),
  installedAppx: new Set(),
  busy: false,
  busyLabel: '',
  /* The bulk job running, if any: only one at a time (the host refuses a second). */
  exclusive: '',
  logOpen: false,
};

/* WinUtil prefixes some categories to sort them ("z__Advanced Tweaks"). */
const displayCategory = (c) => c.replace(/^z__/, '').replace(/^_+/, '');

/* --------------------------------------------------------------------- tabs */

function showTab(name) {
  state.tab = name;
  document.querySelectorAll('.tab').forEach((t) =>
    t.classList.toggle('on', t.dataset.tab === name || (name === 'appx' && t.dataset.tab === 'tweaks')));
  document.querySelectorAll('.view').forEach((v) => v.classList.toggle('on', v.id === `tab-${name}`));
  if (name === 'appx' && state.installedAppx.size === 0) post({ cmd: 'job', kind: 'detectAppx' });
}

document.querySelectorAll('.tab').forEach((t) => t.addEventListener('click', () => showTab(t.dataset.tab)));

$('search').addEventListener('input', (e) => {
  state.query = e.target.value.trim().toLowerCase();
  renderApps();
  renderTweaks();
  renderConfig();
  renderAppx();
});

const matches = (...texts) => !state.query || texts.some((t) => (t || '').toLowerCase().includes(state.query));

/* ------------------------------------------------------------------ install */

const apps = Object.entries(BOOT.apps).map(([key, a]) => ({ key, ...a }))
  .sort((a, b) => a.content.localeCompare(b.content));
const categories = [...new Set(apps.map((a) => a.category))].sort();

function renderChips() {
  const chips = $('chips');
  chips.replaceChildren(...['All', ...categories].map((c) => {
    const b = h('button', `chip${state.category === c ? ' on' : ''}`, c);
    b.addEventListener('click', () => { state.category = c; renderChips(); renderApps(); });
    return b;
  }));
}

const CHECK = '<svg width="10" height="10" viewBox="0 0 10 10" fill="none"><path d="M2 5.2 4.2 7.3 8 3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
const CHEV = '<svg class="chev" width="11" height="11" viewBox="0 0 10 10" fill="none"><path d="M2 3.5 5 6.5 8 3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>';

function renderApps() {
  const shown = apps.filter((a) =>
    (state.category === 'All' || a.category === state.category) && matches(a.content, a.description, a.category));
  const groups = categories.map((c) => [c, shown.filter((a) => a.category === c)]).filter(([, list]) => list.length);

  const sections = groups.map(([cat, list]) => {
    const section = h('div', `category${state.collapsed.has(cat) && !state.query ? ' closed' : ''}`);
    const head = h('button', 'cat-head');
    head.innerHTML = CHEV;
    head.append(h('span', 'heading', cat), h('span', 'count', `${list.length}`));
    head.addEventListener('click', () => {
      state.collapsed.has(cat) ? state.collapsed.delete(cat) : state.collapsed.add(cat);
      renderApps();
    });
    const cards = h('div', 'cards');
    for (const a of list) cards.append(appCard(a));
    section.append(head, cards);
    return section;
  });
  $('apps').replaceChildren(...(sections.length ? sections : [h('div', 'empty-search', 'No apps match.')]));
  renderSelection();
}

/* Which package an install fetches, and from where, for the card's tooltip. */
function packageLine(a) {
  const choco = BOOT.settings.packageManager === 'choco';
  const raw = (choco ? a.choco : a.winget) || '';
  const id = raw.split(';').pop().trim();
  if (!id || id === 'na') return `Not available with ${choco ? 'Chocolatey' : 'WinGet'}.`;
  if (choco) return `Installs with Chocolatey: ${id} (community.chocolatey.org/packages/${id})`;
  if (id.startsWith('msstore:')) return `Installs with WinGet from the Microsoft Store: ${id.slice(8)}`;
  return `Installs with WinGet: ${id}, from its publisher's own download`;
}

/* Where an entry downloads from while it runs, as a line under it. */
function sourceLine(key) {
  const d = BOOT.downloads && BOOT.downloads[key];
  if (!d) return null;
  const s = h('span', 'src', `↓ Downloads from ${d.who}`);
  s.title = `${d.url}\nClick to open the address.`;
  s.addEventListener('click', () => post({ cmd: 'openUrl', url: d.url }));
  return s;
}

const comingSoon = (key) => (BOOT.comingSoon || []).includes(key);

function appCard(a) {
  const card = h('button', `appcard${state.selectedApps.has(a.key) ? ' sel' : ''}`);
  card.title = `${a.description || ''}\n\n${packageLine(a)}${a.link ? `\nWebsite: ${a.link}\nDouble-click to open its website.` : ''}`;
  const icon = BOOT.icons && BOOT.icons[a.key];
  if (icon) {
    const tile = h('span', 'tile icon');
    const img = h('img');
    img.src = icon; // built in; nothing is loaded from the internet
    img.alt = '';
    tile.append(img);
    card.append(tile);
  } else {
    card.append(h('span', 'tile', a.content[0]));
  }
  const name = h('span', 'name', a.content);
  if (a.foss) name.append(h('span', 'foss-dot'));
  card.append(name);
  if (state.installedApps.has(a.key)) card.append(h('span', 'installed', 'installed'));
  const check = h('span', 'check');
  check.innerHTML = CHECK;
  card.append(check);
  card.addEventListener('click', () => {
    state.selectedApps.has(a.key) ? state.selectedApps.delete(a.key) : state.selectedApps.add(a.key);
    card.classList.toggle('sel', state.selectedApps.has(a.key));
    renderSelection();
  });
  card.addEventListener('dblclick', () => a.link && post({ cmd: 'openUrl', url: a.link }));
  return card;
}

function renderSelection() {
  $('sel-count').textContent = `Selected Apps: ${state.selectedApps.size}`;
  const names = [...state.selectedApps].map((k) => BOOT.apps[k]?.content || k).sort();
  $('sel-list').replaceChildren(...names.map((n) => h('div', null, n)));
}

function selectedApps() {
  if (!state.selectedApps.size) {
    logLine('warn', 'Select at least one app first.');
    openLog(true);
    return null;
  }
  return [...state.selectedApps];
}

const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
const byName = (a, b) => a.localeCompare(b);

$('act-install').addEventListener('click', () => {
  const keys = selectedApps();
  if (keys && notBusy()) post({ cmd: 'job', kind: 'install', keys });
});
$('act-uninstall').addEventListener('click', () => {
  const keys = selectedApps();
  if (!keys || !notBusy()) return;
  confirmThen('Uninstall these apps?', `${plural(keys.length, 'app')} will be removed with ${pmName()}:`,
    () => post({ cmd: 'job', kind: 'uninstall', keys }),
    { items: keys.map((k) => BOOT.apps[k]?.content || k).sort(byName) });
});
$('act-upgrade').addEventListener('click', () => notBusy() && post({ cmd: 'job', kind: 'upgradeAll' }));
$('sel-clear').addEventListener('click', () => { state.selectedApps.clear(); renderApps(); });
$('sel-collapse').addEventListener('click', () => { categories.forEach((c) => state.collapsed.add(c)); renderApps(); });
$('sel-expand').addEventListener('click', () => { state.collapsed.clear(); renderApps(); });
$('sel-count').addEventListener('click', () => $('sel-list').classList.toggle('on'));
$('sel-installed').addEventListener('click', () => post({ cmd: 'job', kind: 'detectApps' }));

const pmName = () => (BOOT.settings.packageManager === 'choco' ? 'Chocolatey' : 'WinGet');
document.querySelectorAll('input[name=pm]').forEach((r) => {
  r.checked = r.value === BOOT.settings.packageManager;
  r.addEventListener('change', () => r.checked && post({ cmd: 'settings', packageManager: r.value }));
});

/* --------------------------------------------- tweaks and config, from JSON */

/* WinUtil lays its config out in two panels; within a panel, categories in name
   order, and in each category the checkboxes and toggles first, then buttons
   and drop-downs. */
function panelGroups(config, panel, skip = () => false) {
  const entries = Object.entries(config)
    .map(([key, e]) => ({ key, ...e }))
    .filter((e) => String(e.panel) === String(panel) && !skip(e));
  const cats = [...new Set(entries.map((e) => e.category))].sort();
  const rank = (e) => ({ Toggle: 0, undefined: 0, Button: 1, Combobox: 2 }[e.Type] ?? 0);
  return cats.map((cat) => [cat, entries.filter((e) => e.category === cat)
    .sort((a, b) => rank(a) - rank(b) || a.Content.localeCompare(b.Content))]);
}

/* Every checkbox of a config in on-screen order, whatever the search is showing. */
const checkboxOrder = (config) => [1, 2]
  .flatMap((p) => panelGroups(config, p).flatMap(([, list]) => list))
  .filter((e) => e.Type === undefined)
  .map((e) => e.key);
const tweakOrder = checkboxOrder(BOOT.tweaks);
const featureOrder = checkboxOrder(BOOT.features);
const tickedIn = (order) => order.filter((k) => state.ticked.has(k));

/* Ticks exactly `keys` among one config's checkboxes. */
function setTicked(order, keys) {
  order.forEach((k) => state.ticked.delete(k));
  keys.filter((k) => order.includes(k)).forEach((k) => state.ticked.add(k));
}

const DNS_HELP = 'Applied when you press Run Tweaks, after the restore point if that is ticked. '
  + 'Default leaves DNS as it is; DHCP goes back to the automatic DNS your network hands out.';

function helpMark(text) {
  const q = h('span', 'help', '?');
  q.title = text || '';
  return q;
}

function tweakItem(e) {
  const row = h('div', 'item');
  row.title = e.Description || '';
  if (e.Type === 'Toggle') {
    const sw = h('input', 'switch');
    sw.type = 'checkbox';
    sw.checked = !!state.toggles[e.key];
    sw.disabled = !(e.key in state.toggles);
    sw.addEventListener('change', () => {
      sw.disabled = true;
      post({ cmd: 'job', kind: 'toggle', key: e.key, on: sw.checked });
    });
    row.append(sw, h('span', 'label', e.Content));
    return row;
  }
  if (e.Type === 'Button') {
    const b = h('button', 'btn action', e.Content);
    b.title = e.Description || '';
    // Kept in its place, switched off until it's built.
    if (comingSoon(e.key)) {
      b.disabled = true;
      b.classList.add('soonbtn');
      b.append(h('span', 'soon', 'Coming soon'));
      b.title = `${e.Description || ''}\n\nComing soon.`;
      return b;
    }
    b.addEventListener('click', () => runButton(e));
    const src = sourceLine(e.key);
    if (!src) return b;
    const wrap = h('div', 'btnwrap');
    wrap.append(b, src);
    return wrap;
  }
  if (e.Type === 'Combobox') {
    row.append(h('span', 'label', e.Content));
    const sel = h('select');
    const items = e.key === 'WPFchangedns' ? e.ComboItems.split(' ') : e.ComboItems.split('|');
    for (const it of items) {
      const o = h('option', null, it.replace(/_/g, ' '));
      o.value = it;
      sel.append(o);
    }
    // DNS only takes effect with Run Tweaks, as in WinUtil; the other drop-downs apply at once.
    const dns = e.key === 'WPFchangedns';
    sel.value = dns ? state.dns : (state.combos[e.key] || items[0]);
    sel.addEventListener('change', () => {
      if (dns) state.dns = sel.value;
      else post({ cmd: 'job', kind: 'combo', key: e.key, state: sel.value });
    });
    row.append(sel, helpMark(dns ? DNS_HELP : e.Description));
    if (dns) sel.title = DNS_HELP;
    if (e.ComboDescriptions) sel.title = Object.entries(e.ComboDescriptions).map(([k, v]) => `${k}: ${v}`).join('\n');
    return row;
  }
  // A checkbox.
  const box = h('input');
  box.type = 'checkbox';
  box.dataset.key = e.key;
  box.checked = state.ticked.has(e.key);
  box.addEventListener('change', () => (box.checked ? state.ticked.add(e.key) : state.ticked.delete(e.key)));
  const label = h('label', 'label', e.Content);
  const id = `cb-${e.key}`;
  box.id = id;
  label.htmlFor = id;
  row.append(box, label, helpMark(e.Description));
  if (state.appliedTweaks.has(e.key)) row.classList.add('applied');
  const src = sourceLine(e.key);
  if (!src) return row;
  const wrap = h('div');
  wrap.append(row, src);
  return wrap;
}

function renderGroups(target, groups) {
  target.replaceChildren(...groups.map(([cat, list]) => {
    const visible = list.filter((e) => matches(e.Content, e.Description));
    if (!visible.length) return h('span');
    const g = h('div', 'group');
    g.append(h('h3', 'heading', displayCategory(cat)));
    for (const e of visible) g.append(tweakItem(e));
    return g;
  }));
}

function renderTweaks() {
  renderGroups($('tweaks-1'), panelGroups(BOOT.tweaks, 1));
  renderGroups($('tweaks-2'), panelGroups(BOOT.tweaks, 2));
}

document.querySelectorAll('[data-preset]').forEach((b) => b.addEventListener('click', () => {
  setTicked(tweakOrder, BOOT.presets[b.dataset.preset] || []);
  renderTweaks();
}));
$('tw-clear').addEventListener('click', () => { setTicked(tweakOrder, []); renderTweaks(); });
$('tw-detect').addEventListener('click', () => post({ cmd: 'job', kind: 'detectTweaks' }));
$('tw-appx').addEventListener('click', () => showTab('appx'));
$('tw-run').addEventListener('click', () => {
  // Ticked tweaks run even if the search is hiding them; the host takes the restore point first.
  const keys = tickedIn(tweakOrder);
  if (!keys.length && state.dns === 'Default') {
    return logLine('warn', 'Tick at least one tweak, or pick a DNS provider, first.'), openLog(true);
  }
  if (notBusy()) post({ cmd: 'job', kind: 'runTweaks', keys, dns: state.dns });
});
$('tw-undo').addEventListener('click', () => {
  const keys = tickedIn(tweakOrder);
  if (!keys.length) return logLine('warn', 'Tick the tweaks to undo first.'), openLog(true);
  if (!notBusy()) return;
  confirmThen('Undo these tweaks?', `${plural(keys.length, 'tweak')} will be put back to Windows' defaults:`,
    () => post({ cmd: 'job', kind: 'undoTweaks', keys }),
    { items: keys.map((k) => BOOT.tweaks[k].Content) });
});

/* Fixes that change much more than their names say: each asks first, saying what it does. */
const CONFIRM_BUTTON = {
  WPFFixesUpdate: ['Reset Windows Update?',
    'This is a heavy reset. Besides Windows Update itself, it deletes every Windows policy setting in the registry '
    + '(including any from your work or school, and many of the tweaks applied here), resets the local security '
    + 'policy, deletes the local Group Policy folders, and resets the network stack and proxy settings. '
    + 'Restart Windows afterwards.'],
  WPFFixesNetwork: ['Reset the network?',
    'Resets Winsock and TCP/IP to their defaults, which can remove static IP addresses and other custom network '
    + 'settings. Restart Windows afterwards.'],
  WPFRemoveUltPerf: ['Restore the default power plans?',
    "This puts back Windows' default power plans and deletes every other one, including any your PC's maker "
    + 'added, not just Ultimate Performance.'],
};

/* Buttons in tweaks and config call one of WinUtil's functions on the host. */
function runButton(e) {
  if (e.key === 'WPFFeatureInstall') {
    const keys = tickedIn(featureOrder);
    if (!keys.length) return logLine('warn', 'Tick the features to install first.'), openLog(true);
    if (notBusy()) post({ cmd: 'job', kind: 'features', keys });
    return;
  }
  if (e.category === 'Legacy Windows Panels' && !e.function) return post({ cmd: 'panel', key: e.key });
  if (!notBusy()) return;
  const go = () => post({ cmd: 'job', kind: 'function', key: e.key, label: e.Content });
  const ask = CONFIRM_BUTTON[e.key];
  if (ask) confirmThen(ask[0], ask[1], go);
  else go();
}

function renderConfig() {
  renderGroups($('config-1'), panelGroups(BOOT.features, 1));
  renderGroups($('config-2'), panelGroups(BOOT.features, 2));
}

/* --------------------------------------------------------------------- appx */

const appx = Object.entries(BOOT.appx).map(([key, a]) => ({ key, ...a }));

function renderAppx() {
  const cats = [...new Set(appx.map((a) => a.Category))].sort();
  $('appx').replaceChildren(...cats.map((cat) => {
    const list = appx.filter((a) => a.Category === cat && matches(a.Content, a.Description, a.PackageId))
      .sort((a, b) => a.Content.localeCompare(b.Content));
    if (!list.length) return h('span');
    const g = h('div', 'group');
    g.append(h('h3', 'heading', cat));
    for (const a of list) {
      const row = h('div', 'item appx-row');
      row.title = `${a.Description || ''}\n${a.PackageId}`;
      const box = h('input');
      box.type = 'checkbox';
      box.id = `ax-${a.key}`;
      box.checked = state.selectedAppx.has(a.key);
      box.addEventListener('change', () => (box.checked ? state.selectedAppx.add(a.key) : state.selectedAppx.delete(a.key)));
      const label = h('label', 'label', a.Content);
      label.htmlFor = box.id;
      if (state.installedAppx.has(a.key)) label.append(h('span', 'installed', 'installed'));
      row.append(box, label, helpMark(a.Description));
      if (a.StoreId) {
        // Reinstall Selected gets it from here.
        const url = `https://apps.microsoft.com/detail/${a.StoreId}`;
        const store = h('span', 'store', 'Store page ↗');
        store.title = `Reinstalls from the Microsoft Store:\n${url}`;
        store.addEventListener('click', () => post({ cmd: 'openUrl', url }));
        row.append(store);
      }
      g.append(row);
    }
    return g;
  }));
}

$('ax-back').addEventListener('click', () => showTab('tweaks'));
$('ax-default').addEventListener('click', () => { state.selectedAppx = new Set(BOOT.presets.AppxDefault || []); renderAppx(); });
$('ax-all').addEventListener('click', () => { state.selectedAppx = new Set(appx.map((a) => a.key)); renderAppx(); });
$('ax-clear').addEventListener('click', () => { state.selectedAppx.clear(); renderAppx(); });
$('ax-detect').addEventListener('click', () => post({ cmd: 'job', kind: 'detectAppx' }));
$('ax-remove').addEventListener('click', () => {
  const keys = [...state.selectedAppx];
  if (!keys.length) return logLine('warn', 'Tick the AppX packages to remove first.'), openLog(true);
  if (!notBusy()) return;
  confirmThen('Remove these apps for every user?',
    `${plural(keys.length, 'built-in app')} will be removed. Reinstall Selected can bring them back from the Microsoft Store:`,
    () => post({ cmd: 'job', kind: 'removeAppx', keys }),
    { items: keys.map((k) => BOOT.appx[k]?.Content || k).sort(byName) });
});
$('ax-reinstall').addEventListener('click', () => {
  const keys = [...state.selectedAppx];
  if (!keys.length) return logLine('warn', 'Tick the AppX packages to reinstall first.'), openLog(true);
  if (notBusy()) post({ cmd: 'job', kind: 'reinstallAppx', keys });
});

/* ------------------------------------------------------------------ updates */

const UPDATE_LABELS = { recommended: 'Applying the Recommended update profile', default: 'Restoring Windows Update defaults', disable: 'Disabling Windows Update' };
document.querySelectorAll('[data-update]').forEach((b) => b.addEventListener('click', () => {
  if (!notBusy()) return;
  const kind = b.dataset.update;
  const go = () => post({ cmd: 'job', kind: 'function', key: kind, label: UPDATE_LABELS[kind] });
  if (kind === 'disable') {
    confirmThen('Disable Windows Update?', 'Disabling Windows Update stops update services, disables scheduled tasks, and clears downloaded update files. Security updates will not be installed until defaults are restored.', go);
  } else {
    go();
  }
}));

/* ------------------------------------------------------------ log and status */

function logLine(kind, text) {
  const log = $('log');
  const atEnd = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
  const row = h('div', `line ${kind}`);
  row.append(h('span', 't', new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })), h('span', 'x', text));
  log.append(row);
  while (log.childElementCount > 2000) log.firstChild.remove();
  if (atEnd) log.scrollTop = log.scrollHeight;
}

function openLog(open) {
  state.logOpen = open;
  $('logwrap').classList.toggle('open', open);
  $('logtoggle').classList.toggle('open', open);
  if (open) $('log').scrollTop = $('log').scrollHeight;
}

$('logtoggle').addEventListener('click', () => openLog(!state.logOpen));

function setStatus(text, lamp) {
  $('statusText').textContent = text;
  $('lamp').className = lamp || '';
}

/* Bulk work runs one job at a time. The host refuses a second one anyway; this
   says so before any question is asked. */
function notBusy() {
  if (!state.exclusive) return true;
  logLine('warn', `${state.exclusive} hasn't finished yet. Wait for it, then try again.`);
  openLog(true);
  return false;
}

/* ------------------------------------------------------------------- dialog */

let dialogGo = null;
/* `items` are listed under the text, so a question says exactly what it will act on. */
function confirmThen(title, text, go, { items = [], ok = 'Continue', cancel = 'Cancel', danger = true } = {}) {
  $('dlg-title').textContent = title;
  $('dlg-text').textContent = text;
  $('dlg-list').replaceChildren(...items.map((n) => h('li', null, n)));
  $('dlg-list').classList.toggle('on', items.length > 0);
  $('dlg-ok').textContent = ok;
  $('dlg-ok').classList.toggle('danger', danger);
  $('dlg-ok').classList.toggle('primary', !danger);
  $('dlg-cancel').textContent = cancel;
  dialogGo = go;
  $('dialogwrap').classList.add('on');
}
$('dlg-cancel').addEventListener('click', () => $('dialogwrap').classList.remove('on'));
$('dlg-ok').addEventListener('click', () => {
  $('dialogwrap').classList.remove('on');
  if (dialogGo) dialogGo();
});

/* -------------------------------------------------------------------- host */

window.onHost = (msg) => {
  switch (msg.type) {
    case 'log':
      logLine(msg.kind, msg.text);
      if (msg.kind === 'error' || msg.open) openLog(true);
      break;
    case 'busy':
      state.busy = true;
      state.busyLabel = msg.label;
      if (msg.exclusive) state.exclusive = msg.label;
      setStatus(`${msg.label}…`, 'busy');
      $('progress').classList.remove('on');
      openLog(true);
      break;
    case 'progress':
      $('progress').classList.toggle('on', msg.total > 1);
      $('bar').style.width = `${msg.total ? Math.round((msg.done / msg.total) * 100) : 0}%`;
      break;
    case 'done':
      state.busy = false;
      if (msg.exclusive) state.exclusive = '';
      $('progress').classList.remove('on');
      setStatus(msg.ok ? `${msg.label}: done` : `${msg.label}: finished with errors (see the log)`, msg.ok ? '' : 'bad');
      break;
    case 'idle':
      state.busy = false;
      state.busyLabel = '';
      state.exclusive = '';
      break;
    case 'closeWhileBusy':
      // ✕, Alt+F4 or the launcher's Stop while a job runs. Closing anyway would cut it off.
      confirmThen('RustWinUtil is still working',
        `${state.busyLabel || 'A job'} hasn't finished. Closing now would stop it partway, which could leave Windows `
        + 'half changed, and anything queued after it would not run. RustWinUtil can hide its window now and close '
        + 'by itself once the work is done.',
        () => post({ cmd: 'window', action: 'closeWhenDone' }),
        { ok: 'Close when done', cancel: 'Keep open', danger: false });
      break;
    case 'installedApps':
      state.installedApps = new Set(msg.keys);
      msg.keys.forEach((k) => state.selectedApps.add(k));
      renderApps();
      break;
    case 'appliedTweaks':
      state.appliedTweaks = new Set(msg.keys);
      setTicked(tweakOrder, msg.keys);
      renderTweaks();
      break;
    case 'states':
      state.toggles = msg.toggles;
      state.combos = msg.combos;
      renderTweaks();
      break;
    case 'installedAppx':
      state.installedAppx = new Set(msg.keys);
      renderAppx();
      break;
    case 'settings':
      BOOT.settings = msg.settings;
      document.documentElement.dataset.theme = msg.settings.theme;
      break;
    case 'window':
      document.documentElement.classList.toggle('maximized', !!msg.maximized);
      break;
  }
};

/* ------------------------------------------------------------------- window */

$('top').addEventListener('mousedown', (e) => {
  if (e.button !== 0 || e.detail > 1) return;
  if (e.target.closest('button, input, select, a')) return;
  post({ cmd: 'window', action: 'drag' });
});
$('top').addEventListener('dblclick', (e) => {
  if (!e.target.closest('button, input, select, a')) post({ cmd: 'window', action: 'maximize' });
});
document.querySelectorAll('.winbtn').forEach((b) => b.addEventListener('click', () => post({ cmd: 'window', action: b.dataset.win })));
document.querySelectorAll('.grip').forEach((g) => g.addEventListener('mousedown', (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  post({ cmd: 'resize', dir: g.dataset.dir });
}));
$('theme').addEventListener('click', () => {
  const theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
  document.documentElement.dataset.theme = theme;
  post({ cmd: 'settings', theme });
});
document.addEventListener('contextmenu', (e) => { if (!e.target.closest('input[type=text], #log')) e.preventDefault(); });
/* Nothing is ever dropped here: a dropped file or link would otherwise replace this page.
   The host turns outside drops off as well. */
['dragover', 'drop'].forEach((type) => document.addEventListener(type, (e) => {
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'none';
}));
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && $('dialogwrap').classList.contains('on')) $('dialogwrap').classList.remove('on');
  if (e.ctrlKey && e.key.toLowerCase() === 'f') { e.preventDefault(); $('search').focus(); }
});

/* --------------------------------------------------------------------- boot */

$('version').textContent = `v${BOOT.version}`;
$('notadmin').classList.toggle('on', !BOOT.elevated);
renderChips();
renderApps();
renderTweaks();
renderConfig();
renderAppx();
logLine('info', `RustWinUtil ${BOOT.version}: ${apps.length} apps, ${Object.keys(BOOT.tweaks).length} tweaks and settings, ${Object.keys(BOOT.features).length} features and fixes. Nothing is fetched from WinUtil's servers.`);
requestAnimationFrame(() => post({ cmd: 'ready' }));
