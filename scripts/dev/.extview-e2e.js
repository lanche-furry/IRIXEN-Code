(async () => {
  const inv = window.__TAURI_INTERNALS__.invoke;
  const wait = (ms) => new Promise(r => setTimeout(r, ms));
  const snap = () => [...document.querySelectorAll('.ext-view-section')].map(s => ({
    view: s.dataset.view,
    name: (s.querySelector('.ext-view-name') || {}).textContent || '',
    rows: [...s.querySelectorAll('.ext-view-row')].map(r => ({
      text: r.textContent.trim().slice(0, 60),
      context: r.dataset.context,
      icon: !!r.querySelector('.ext-view-icon, .ext-view-icon-img'),
    })),
    empty: (s.querySelector('.ext-view-empty') || {}).textContent || '',
  }));
  for (let i = 0; i < 30 && !document.querySelector('.act-ext'); i++) await wait(300);
  const btn = document.querySelector('.act-btn[data-view="ext:gradleContainerView"]');
  if (!btn) return { error: 'no activity button' };
  btn.click();
  await wait(4000);
  const initial = snap();
  for (const s of [...document.querySelectorAll('.ext-view-section')]) {
    if (!s.querySelector('.ext-view-body').children.length) s.querySelector('.ext-view-toggle').click();
  }
  await wait(3500);
  const expanded = snap();

  const first = document.querySelector('.ext-view-section');
  const more = first && first.querySelectorAll('.ext-view-tool')[1];
  if (more) more.click();
  await wait(250);
  const titleItems = [...document.querySelectorAll('.irixen-popup [role="menuitem"] .fname')].map(e => e.textContent);
  document.querySelectorAll('.irixen-popup').forEach(n => n.remove());

  const row = document.querySelector('.ext-view-row');
  let contextItems = [];
  if (row) {
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 200, clientY: 300 }));
    await wait(250);
    contextItems = [...document.querySelectorAll('.irixen-popup [role="menuitem"] .fname')].map(e => e.textContent);
    document.querySelectorAll('.irixen-popup').forEach(n => n.remove());
  }
  let clickErr = null;
  try { await inv('exttree_command', { view: 'gradleTasksView', key: row && row.dataset.key, command: null }); }
  catch (e) { clickErr = String(e); }

  const logs = await inv('exthost_logs', {});
  return {
    initial, expanded,
    titleCount: titleItems.length, titleItems: titleItems.slice(0, 8),
    contextCount: contextItems.length, contextItems: contextItems.slice(0, 8),
    clickErr,
    errors: logs.filter(l => l.level === 'err').slice(-6).map(l => l.text),
    logTotal: logs.length,
  };
})()
