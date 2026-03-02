// DD Notebook Enhance — bookmarklet for Datadog notebooks
//
// Features:
//   1. Resolves section links: [text](#slug) → ?cell_id= URLs
//   2. Creates annotations from an ## Annotations section
//
// Annotation format (in your notebook markdown):
//
//   ## Annotations
//   - 2026-02-05 13:00 UTC | red | Regression onset
//   - 2026-02-06 09:00 UTC | gray | Deploy abc123
//   - 2026-02-07 15:30 UTC | green | Recovery
//
// Colors: red, yellow, green, blue, purple, pink, orange, gray
//
// Setup:
//   1. Install terser: npm install -g terser
//   2. Generate bookmarklet:
//      terser dd-notebook-enhance.js --compress --mangle \
//        | tr -d '\n' | sed 's/;$//' \
//        | { echo -n 'javascript:void('; cat; echo -n ')'; } \
//        | pbcopy
//   3. Create a bookmark in Chrome, paste the clipboard as the URL
//   4. Navigate to a Datadog notebook and click the bookmarklet
//
// Idempotent: annotations that already exist (same timestamp + description)
// are skipped. Section links already resolved are left alone. Safe to run
// multiple times.

(() => {
  const COLORS = {
    red: '#d73d42',
    yellow: '#f5c840',
    green: '#2da769',
    blue: '#4c80f0',
    purple: '#7a4eda',
    pink: '#d84489',
    orange: '#e5803e',
    gray: '#828ba4',
  };
  const norm = s => s.replace(/-+/g, '-').replace(/^-|-$/g, '');

  const editor = document.querySelector('.tiptap.ProseMirror');
  if (!editor) { alert('No editor found'); return; }
  const notebookId = window.location.pathname.match(/\/notebook\/(\d+)/)?.[1];
  if (!notebookId) { alert('Not on a notebook page'); return; }

  // ── 1. Resolve section links ──────────────────────────────────────────
  const view = editor.editor.view;
  const headings = {};
  editor.querySelectorAll('h1[data-cell],h2[data-cell],h3[data-cell],h4[data-cell],h5[data-cell],h6[data-cell]').forEach(el => {
    const clone = el.cloneNode(true);
    clone.querySelectorAll('.HeaderLink, .ProseMirror-widget, .ProseMirror-separator, .ProseMirror-trailingBreak').forEach(n => n.remove());
    const text = clone.textContent.trim();
    const slug = norm(text.toLowerCase().replace(/[^a-z0-9]+/g, '-'));
    headings[slug] = el.dataset.cell;
  });

  const { state } = view;
  const linkType = state.schema.marks.link;
  let tr = state.tr;
  let linkCount = 0;

  state.doc.descendants((node, pos) => {
    if (!node.isText) return;
    const linkMark = node.marks.find(m => m.type === linkType);
    if (!linkMark) return;
    const href = linkMark.attrs.href;
    let slug;
    if (href.startsWith('#')) {
      slug = href.substring(1);
    } else if (href.includes('#') && href.includes('/notebook/')) {
      slug = href.split('#').pop();
    } else {
      return;
    }
    const normalized = norm(slug);
    if (!headings[normalized]) return;
    const newHref = `https://app.datadoghq.com/notebook/${notebookId}?cell_id=${headings[normalized]}`;
    const newMark = linkType.create({ ...linkMark.attrs, href: newHref });
    tr = tr.removeMark(pos, pos + node.nodeSize, linkMark);
    tr = tr.addMark(pos, pos + node.nodeSize, newMark);
    linkCount++;
  });

  if (tr.docChanged) {
    view.dispatch(tr);
  }

  // ── 2. Create annotations (skip existing) ─────────────────────────────
  const token = document.querySelector('[name="_authentication_token"]')?.value;
  const text = editor.innerText;
  // innerText doesn't have "##" — just the heading text + "Copy link to section"
  const match = text.match(/\bAnnotations\b[^\n]*\n([\s\S]*?)(?=\n[A-Z][^\n]*\nCopy link to section|$)/);

  if (!match) {
    alert(`${linkCount} links resolved, no Annotations section found.`);
    return;
  }

  // Lines may or may not have bullet markers — match by timestamp pattern
  const lines = match[1].split('\n').filter(l => l.trim().match(/^\d{4}-\d{2}-\d{2}|^[-•*]\s*\d{4}-\d{2}-\d{2}/));
  const annotations = [];
  for (const line of lines) {
    const clean = line.replace(/^[\s\-•*]+/, '').trim();
    const parts = clean.split('|').map(s => s.trim());
    if (parts.length < 3) continue;
    const [timestamp, color, ...descParts] = parts;
    const description = descParts.join('|').trim();
    const time = new Date(timestamp);
    if (isNaN(time)) { console.warn('Bad timestamp:', timestamp); continue; }
    const hex = COLORS[color.toLowerCase()] || COLORS.gray;
    annotations.push({ time: time.getTime(), color: hex, description });
  }

  if (!annotations.length) {
    alert(`${linkCount} links resolved, no annotations parsed.`);
    return;
  }

  // Fetch existing annotations for this notebook (wide time range)
  const now = Date.now();
  fetch(`/api/ui/annotation?page_id=notebook:${notebookId}&start_time=${now - 365*24*60*60*1000}&end_time=${now + 24*60*60*1000}`, {
    credentials: 'include',
  })
  .then(r => r.json())
  .then(existing => {
    const existingSet = new Set(
      (existing || []).map(e => `${e.start_time}|${e.description}`)
    );

    const toCreate = annotations.filter(a =>
      !existingSet.has(`${a.time}|${a.description}`)
    );

    const skipped = annotations.length - toCreate.length;
    if (skipped) console.log(`Skipping ${skipped} existing annotations`);

    if (!toCreate.length) {
      const parts = [];
      if (linkCount) parts.push(`${linkCount} links resolved`);
      parts.push(`${skipped} annotations already exist`);
      alert(parts.join(', '));
      return;
    }

    console.log(`Creating ${toCreate.length} annotations...`);
    toCreate.forEach(a => console.log(`  ${new Date(a.time).toISOString()} | ${a.color} | ${a.description}`));

    return Promise.all(toCreate.map(a =>
      fetch('/api/ui/annotation', {
        method: 'POST',
        mode: 'cors',
        credentials: 'include',
        headers: {
          'content-type': 'application/json',
          'x-csrf-token': token,
        },
        body: JSON.stringify({
          widget_ids: null,
          page_id: `notebook:${notebookId}`,
          type: 'pointInTime',
          start_time: a.time,
          description: a.description,
          color: a.color,
          _authentication_token: token,
        })
      }).then(r => r.json())
    )).then(results => {
      const ok = results.filter(r => r.id).length;
      const failed = results.length - ok;
      const parts = [];
      if (linkCount) parts.push(`${linkCount} links resolved`);
      if (ok) parts.push(`${ok} annotations created`);
      if (skipped) parts.push(`${skipped} already existed`);
      if (failed) parts.push(`${failed} failed`);
      if (failed) console.log('Failed:', results.filter(r => !r.id));
      alert(parts.join(', ') + (ok ? '. Page will reload.' : '.'));
      if (ok) location.reload();
    });
  });
})()
