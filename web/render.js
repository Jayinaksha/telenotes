// DOM rendering. Pure(-ish) functions of state passed in by app.js --
// nothing here talks to Supabase or the bot directly. `textContent`
// throughout: item content is arbitrary user text and must never be parsed
// as HTML.

const $ = (id) => document.getElementById(id);

const PANES = ["write", "browse", "tags", "stats"];

/// Switches between the pre-login screens and the signed-in app shell.
/// `id` is "view-auth", "view-link", or "shell".
export function showEntry(id) {
  for (const viewId of ["view-auth", "view-link"]) $(viewId).hidden = viewId !== id;
  $("shell").hidden = id !== "shell";
}

/// Switches which panel is visible inside the app shell, and updates the
/// sidebar's active tab to match.
export function showPane(name) {
  for (const p of PANES) $(`pane-${p}`).hidden = p !== name;
  for (const tab of document.querySelectorAll(".navtab")) {
    tab.classList.toggle("active", tab.dataset.view === name);
  }
}

export function show(el, text) {
  if (text !== undefined) el.textContent = text;
  el.hidden = false;
}

export function flash(el, text, bad = false) {
  el.textContent = text;
  el.classList.toggle("bad", bad);
  el.hidden = false;
}

/// Full tag picker for the Tags pane -- selecting one jumps to Browse
/// filtered by it (handled by the caller's onSelect).
export function renderTagList(tagCounts, onSelect) {
  const box = $("tag-list");
  box.replaceChildren();

  if (tagCounts.length === 0) {
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent = "No tags yet. Add #tags to a note or file caption.";
    box.append(p);
    return;
  }

  for (const { tag, n } of tagCounts) {
    const b = document.createElement("button");
    b.append(`#${tag}`);
    const count = document.createElement("span");
    count.className = "count";
    count.textContent = n;
    b.append(count);
    b.onclick = () => onSelect(tag);
    box.append(b);
  }
}

/// The small "filtering by #tag" pill shown above Browse's results when a
/// tag filter is active.
export function renderTagFilter(tag, onClear) {
  const el = $("tag-filter");
  if (!tag) {
    el.hidden = true;
    return;
  }
  el.replaceChildren();
  el.append(`Filtering by #${tag}`);
  const clear = document.createElement("button");
  clear.textContent = "Clear";
  clear.onclick = onClear;
  el.append(clear);
  el.hidden = false;
}

export function renderItems(items, query, editingId, handlers) {
  const box = $("results");
  box.replaceChildren();

  if (items.length === 0) {
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent = query
      ? `Nothing matches "${query}".`
      : "Nothing here yet. Send your bot a message, or use the forms above.";
    box.append(p);
    return;
  }

  for (const item of items) {
    box.append(item.id === editingId ? renderItemEditing(item, handlers) : renderItem(item, handlers));
  }
}

/// Browsers often send an empty or wrong Content-Type for .md uploads, so
/// fall back to the file extension when the declared mime type doesn't say
/// markdown explicitly.
function isMarkdownDocument(item) {
  if (item.mime_type === "text/markdown") return true;
  return /\.(md|markdown)$/i.test(item.file_name ?? "");
}

/// Same unreliable-mime-type story as markdown -- fall back to the
/// extension for uploaded HTML files.
function isHtmlDocument(item) {
  if (item.mime_type === "text/html") return true;
  return /\.(html|htm)$/i.test(item.file_name ?? "");
}

/// What kind of inline preview an item supports, or null if it's a plain
/// download (e.g. a generic document that isn't a PDF, markdown, or HTML
/// file).
function previewKind(item) {
  if (item.kind === "photo") return "image";
  if (item.kind === "video") return "video";
  if (item.kind === "voice" || item.kind === "audio") return "audio";
  if (item.kind === "document" && item.mime_type === "application/pdf") return "pdf";
  if (item.kind === "document" && isMarkdownDocument(item)) return "markdown";
  if (item.kind === "document" && isHtmlDocument(item)) return "html";
  return null;
}

/// Builds the actual media element for a resolved preview URL. `large`
/// switches to the fullscreen sizing class instead of the inline-card one.
function buildMediaNode(kind, url, item, large) {
  if (kind === "image") {
    const img = document.createElement("img");
    img.src = url;
    img.alt = item.file_name ?? "";
    img.className = large ? "lightbox-media" : "preview-media";
    return img;
  }
  if (kind === "video") {
    const video = document.createElement("video");
    video.src = url;
    video.controls = true;
    video.className = large ? "lightbox-media" : "preview-media";
    return video;
  }
  if (kind === "audio") {
    const audio = document.createElement("audio");
    audio.src = url;
    audio.controls = true;
    audio.className = "preview-media";
    return audio;
  }
  // pdf
  const iframe = document.createElement("iframe");
  iframe.src = url;
  iframe.className = large ? "lightbox-pdf" : "preview-pdf";
  return iframe;
}

function markdownNode(html) {
  const div = document.createElement("div");
  div.className = "rendered-note";
  div.innerHTML = html;
  return div;
}

/// Uploaded HTML is arbitrary, unlike a note's own markdown -- sandboxed
/// with no tokens at all (no scripts, no same-origin, no top-navigation) so
/// even a DOMPurify miss can't execute or reach the dashboard's session.
/// `.srcdoc` is a property assignment, not an attribute string, so it needs
/// no manual escaping.
function htmlDocumentNode(sanitizedHtml, large) {
  const iframe = document.createElement("iframe");
  iframe.sandbox = "";
  iframe.srcdoc = sanitizedHtml;
  iframe.className = large ? "lightbox-html" : "preview-html";
  return iframe;
}

/// A full-viewport overlay for any content -- images, video, rendered
/// markdown. `build()` returns the node to display; called fresh each open
/// so fullscreen views aren't stuck sharing state with the inline preview.
export function openLightbox(build) {
  const overlay = document.createElement("div");
  overlay.className = "lightbox";

  const box = document.createElement("div");
  box.className = "lightbox-content";
  box.append(build());

  const close = document.createElement("button");
  close.className = "ghost small lightbox-close";
  close.textContent = "Close";
  const dismiss = () => {
    overlay.remove();
    document.removeEventListener("keydown", onKey);
  };
  close.onclick = dismiss;
  overlay.onclick = (e) => {
    if (e.target === overlay) dismiss();
  };
  function onKey(e) {
    if (e.key === "Escape") dismiss();
  }
  document.addEventListener("keydown", onKey);

  overlay.append(box, close);
  document.body.append(overlay);
}

function errorNode(message) {
  const p = document.createElement("p");
  p.className = "empty";
  p.textContent = message;
  return p;
}

/// Compact list of semantically-related items, shown inline under a card.
/// `excludeId` guards against an item ever listing itself, though the RPC
/// already excludes it server-side.
function renderRelatedList(box, hits, excludeId) {
  box.replaceChildren();
  const filtered = (hits ?? []).filter((h) => h.id !== excludeId);
  if (filtered.length === 0) {
    box.append(errorNode("Nothing related found yet."));
    return;
  }
  for (const hit of filtered) {
    const row = document.createElement("div");
    row.className = "related-row";

    const kind = document.createElement("span");
    kind.className = "kind";
    kind.textContent = hit.kind;
    row.append(kind);

    const label = document.createElement("span");
    label.className = "related-label";
    label.textContent =
      hit.kind === "note"
        ? (hit.content_text ?? "").split("\n")[0].slice(0, 80) || "(empty note)"
        : hit.file_name ?? `${hit.kind} file`;
    row.append(label);

    row.append(" ");

    const stamp = document.createElement("span");
    stamp.className = "item-stamp";
    stamp.textContent = hit.id.slice(0, 8);
    row.append(stamp);

    box.append(row);
  }
}

function itemHead(item) {
  const head = document.createElement("div");
  head.className = "item-head";

  const kind = document.createElement("span");
  kind.className = "kind";
  kind.textContent = item.kind;
  head.append(kind);

  const stamp = document.createElement("span");
  stamp.className = "item-stamp";
  stamp.textContent = item.id.slice(0, 8);
  head.append(stamp);

  if (item.kind !== "note" && item.file_name) {
    const name = document.createElement("strong");
    name.textContent = item.file_name;
    head.append(name);
  }

  const date = document.createElement("span");
  date.className = "item-date";
  date.textContent = new Date(item.created_at).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
  head.append(date);
  return head;
}

function renderItem(
  item,
  { onDownload, onEdit, onDelete, onRenderNote, onResolveUrl, onRenderDocument, onRenderHtmlDocument, onRelated },
) {
  const el = document.createElement("article");
  el.className = "item";
  el.append(itemHead(item));

  let body = null;
  if (item.content_text) {
    body = document.createElement("p");
    body.className = "item-body";
    body.textContent = item.content_text;
    el.append(body);
  }

  const relatedBox = document.createElement("div");
  relatedBox.className = "item-related";
  relatedBox.hidden = true;
  el.append(relatedBox);

  const pKind = previewKind(item);
  let previewBox = null;
  if (pKind) {
    previewBox = document.createElement("div");
    previewBox.className = "item-preview";
    previewBox.hidden = true;
    el.append(previewBox);
  }

  const actions = document.createElement("div");
  actions.className = "item-actions";

  // Notes: toggle between plain text and rendered markdown+LaTeX in place.
  if (item.kind === "note" && item.content_text && onRenderNote) {
    let rendered = false;
    let html = null;

    const renderBtn = document.createElement("button");
    renderBtn.className = "ghost small";
    renderBtn.textContent = "Render";
    renderBtn.onclick = async () => {
      if (rendered) {
        body.textContent = item.content_text;
        body.classList.remove("rendered-note");
        rendered = false;
        renderBtn.textContent = "Render";
        return;
      }
      if (!html) {
        renderBtn.disabled = true;
        renderBtn.textContent = "Rendering…";
        try {
          html = await onRenderNote(item);
        } catch (e) {
          renderBtn.disabled = false;
          renderBtn.textContent = "Render";
          return show(body, `Couldn't render: ${e.message}`);
        }
        renderBtn.disabled = false;
      }
      body.innerHTML = html;
      body.classList.add("rendered-note");
      rendered = true;
      renderBtn.textContent = "Show raw";
    };
    actions.append(renderBtn);

    const fsBtn = document.createElement("button");
    fsBtn.className = "ghost small";
    fsBtn.textContent = "Fullscreen";
    fsBtn.onclick = async () => {
      if (!html) {
        fsBtn.disabled = true;
        fsBtn.textContent = "Loading…";
        try {
          html = await onRenderNote(item);
        } catch (e) {
          flash($("boot-error"), `Couldn't render: ${e.message}`, true);
          return;
        } finally {
          fsBtn.disabled = false;
          fsBtn.textContent = "Fullscreen";
        }
      }
      const box = document.createElement("div");
      box.className = "rendered-note";
      box.innerHTML = html;
      openLightbox(() => box);
    };
    actions.append(fsBtn);
  }

  // Previewable files: load the content inline on demand, with a fullscreen
  // escape hatch. Markdown and HTML documents each go through their own
  // fetch-then-convert pipeline (raw text -> rendered/sanitized string,
  // via `textPreview.load`); everything else resolves to a signed URL and
  // gets a native media element.
  const textPreview =
    pKind === "markdown"
      ? { load: onRenderDocument, build: markdownNode }
      : pKind === "html"
        ? { load: onRenderHtmlDocument, build: htmlDocumentNode }
        : null;

  if (textPreview || (pKind && onResolveUrl)) {
    let url = null;
    let content = null;

    const previewBtn = document.createElement("button");
    previewBtn.className = "ghost small";
    previewBtn.textContent = "Preview";
    previewBtn.onclick = async () => {
      if (url || content) {
        previewBox.hidden = !previewBox.hidden;
        previewBtn.textContent = previewBox.hidden ? "Preview" : "Hide preview";
        return;
      }
      previewBtn.disabled = true;
      previewBtn.textContent = "Loading…";
      try {
        if (textPreview) {
          content = await textPreview.load(item);
          previewBox.replaceChildren(textPreview.build(content, false));
        } else {
          url = await onResolveUrl(item);
          previewBox.replaceChildren(buildMediaNode(pKind, url, item, false));
        }
        previewBox.hidden = false;
        previewBtn.textContent = "Hide preview";
      } catch (e) {
        previewBox.replaceChildren(errorNode(`Couldn't load preview: ${e.message}`));
        previewBox.hidden = false;
        previewBtn.textContent = "Preview";
      } finally {
        previewBtn.disabled = false;
      }
    };
    actions.append(previewBtn);

    const fsBtn = document.createElement("button");
    fsBtn.className = "ghost small";
    fsBtn.textContent = "Fullscreen";
    fsBtn.onclick = async () => {
      fsBtn.disabled = true;
      const originalText = fsBtn.textContent;
      fsBtn.textContent = "Loading…";
      try {
        if (textPreview) {
          if (!content) content = await textPreview.load(item);
          openLightbox(() => textPreview.build(content, true));
        } else {
          if (!url) url = await onResolveUrl(item);
          openLightbox(() => buildMediaNode(pKind, url, item, true));
        }
      } catch (e) {
        flash($("boot-error"), `Couldn't open fullscreen: ${e.message}`, true);
      } finally {
        fsBtn.disabled = false;
        fsBtn.textContent = originalText;
      }
    };
    actions.append(fsBtn);
  }

  if (item.kind !== "note" && item.storage_path) {
    const btn = document.createElement("button");
    btn.className = "download";
    btn.textContent = "Download";
    btn.onclick = () => onDownload(item, btn);
    actions.append(btn);
  }

  if (onRelated) {
    const relatedBtn = document.createElement("button");
    relatedBtn.className = "ghost small";
    relatedBtn.textContent = "Related";
    relatedBtn.onclick = async () => {
      if (!relatedBox.dataset.loaded) {
        relatedBtn.disabled = true;
        relatedBtn.textContent = "Loading…";
        try {
          const hits = await onRelated(item);
          renderRelatedList(relatedBox, hits, item.id);
          relatedBox.dataset.loaded = "1";
        } catch (e) {
          relatedBox.replaceChildren(errorNode(`Couldn't load related items: ${e.message}`));
        } finally {
          relatedBtn.disabled = false;
          relatedBtn.textContent = "Related";
        }
      }
      relatedBox.hidden = !relatedBox.hidden;
    };
    actions.append(relatedBtn);
  }

  const editBtn = document.createElement("button");
  editBtn.className = "ghost small";
  editBtn.textContent = "Edit";
  editBtn.onclick = () => onEdit(item);
  actions.append(editBtn);

  const deleteBtn = document.createElement("button");
  deleteBtn.className = "ghost small danger-text";
  deleteBtn.textContent = "Delete";
  deleteBtn.onclick = () => onDelete(item, deleteBtn);
  actions.append(deleteBtn);

  el.append(actions);

  if (item.tags?.length) {
    const tags = document.createElement("div");
    tags.className = "item-tags";
    for (const t of item.tags) {
      const s = document.createElement("span");
      s.textContent = `#${t}`;
      tags.append(s);
    }
    el.append(tags);
  }

  return el;
}

function renderItemEditing(item, { onSaveEdit, onCancelEdit }) {
  const el = document.createElement("article");
  el.className = "item editing";
  el.append(itemHead(item));

  const textArea = document.createElement("textarea");
  textArea.className = "edit-field";
  textArea.rows = 3;
  textArea.value = item.content_text ?? "";
  el.append(textArea);

  const tagsInput = document.createElement("input");
  tagsInput.className = "edit-field";
  tagsInput.placeholder = "tags, comma, separated";
  tagsInput.value = (item.tags ?? []).join(", ");
  el.append(tagsInput);

  let fileNameInput = null;
  let mimeInput = null;
  if (item.kind !== "note") {
    fileNameInput = document.createElement("input");
    fileNameInput.className = "edit-field";
    fileNameInput.placeholder = "file name";
    fileNameInput.value = item.file_name ?? "";
    el.append(fileNameInput);

    mimeInput = document.createElement("input");
    mimeInput.className = "edit-field";
    mimeInput.placeholder = "mime type";
    mimeInput.value = item.mime_type ?? "";
    el.append(mimeInput);
  }

  const actions = document.createElement("div");
  actions.className = "item-actions";

  const save = document.createElement("button");
  save.textContent = "Save";
  save.onclick = () =>
    onSaveEdit(item, {
      content_text: textArea.value,
      tags: tagsInput.value
        .split(",")
        .map((t) => t.trim().toLowerCase())
        .filter(Boolean),
      file_name: fileNameInput ? fileNameInput.value || null : null,
      mime_type: mimeInput ? mimeInput.value || null : null,
    });
  actions.append(save);

  const cancel = document.createElement("button");
  cancel.className = "ghost";
  cancel.textContent = "Cancel";
  cancel.onclick = () => onCancelEdit();
  actions.append(cancel);

  el.append(actions);
  return el;
}

export function renderStats(tagCounts, days, storage) {
  const totalNotes = storage
    .filter((s) => s.kind === "note")
    .reduce((sum, s) => sum + Number(s.n), 0);
  const totalFiles = storage
    .filter((s) => s.kind !== "note")
    .reduce((sum, s) => sum + Number(s.n), 0);
  const totalBytes = storage.reduce((sum, s) => sum + Number(s.bytes), 0);

  const tiles = $("stats-tiles");
  tiles.replaceChildren();
  for (const [label, value] of [
    ["Notes", totalNotes],
    ["Files", totalFiles],
    ["Tags", tagCounts.length],
    ["Storage", `${(totalBytes / 1024 / 1024).toFixed(1)} MB`],
  ]) {
    const tile = document.createElement("div");
    tile.className = "stat-tile";
    const num = document.createElement("div");
    num.className = "stat-value";
    num.textContent = value;
    const lbl = document.createElement("div");
    lbl.className = "stat-label";
    lbl.textContent = label;
    tile.append(num, lbl);
    tiles.append(tile);
  }

  const activity = $("stats-activity");
  activity.replaceChildren();
  const maxN = Math.max(1, ...days.map((d) => Number(d.n)));
  for (const d of days) {
    const row = document.createElement("div");
    row.className = "activity-row";
    const label = document.createElement("span");
    label.className = "activity-day";
    label.textContent = d.day;
    const bar = document.createElement("span");
    bar.className = "activity-bar";
    bar.style.width = `${(Number(d.n) / maxN) * 100}%`;
    const count = document.createElement("span");
    count.className = "activity-count";
    count.textContent = d.n;
    row.append(label, bar, count);
    activity.append(row);
  }
  if (days.length === 0) {
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent = "No activity in the last 30 days.";
    activity.append(p);
  }

  const table = $("stats-storage");
  table.replaceChildren();
  for (const s of storage) {
    const row = document.createElement("div");
    row.className = "storage-row";
    row.textContent = `${s.kind} / ${s.backend}: ${s.n} (${(Number(s.bytes) / 1024 / 1024).toFixed(1)} MB)`;
    table.append(row);
  }
  if (storage.length === 0) {
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent = "Nothing stored yet.";
    table.append(p);
  }
}
