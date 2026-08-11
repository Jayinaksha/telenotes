// DOM rendering. Pure(-ish) functions of state passed in by app.js --
// nothing here talks to Supabase or the bot directly. `textContent`
// throughout: item content is arbitrary user text and must never be parsed
// as HTML.

const $ = (id) => document.getElementById(id);

export function only(viewId) {
  for (const id of ["view-auth", "view-link", "view-main", "view-stats"]) {
    $(id).hidden = id !== viewId;
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

export function renderTags(tagCounts, selectedTag, onSelect) {
  const box = $("tags");
  box.replaceChildren();

  const all = document.createElement("button");
  all.textContent = "All tags";
  all.classList.toggle("active", selectedTag === null);
  all.onclick = () => onSelect(null);
  box.append(all);

  for (const { tag, n } of tagCounts) {
    const b = document.createElement("button");
    b.classList.toggle("active", selectedTag === tag);
    b.append(`#${tag}`);
    const count = document.createElement("span");
    count.className = "count";
    count.textContent = n;
    b.append(count);
    b.onclick = () => onSelect(selectedTag === tag ? null : tag);
    box.append(b);
  }
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

function itemHead(item) {
  const head = document.createElement("div");
  head.className = "item-head";

  const kind = document.createElement("span");
  kind.className = "kind";
  kind.textContent = item.kind;
  head.append(kind);

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

function renderItem(item, { onDownload, onEdit, onDelete }) {
  const el = document.createElement("article");
  el.className = "item";
  el.append(itemHead(item));

  if (item.content_text) {
    const body = document.createElement("p");
    body.className = "item-body";
    body.textContent = item.content_text;
    el.append(body);
  }

  const actions = document.createElement("div");
  actions.className = "item-actions";

  if (item.kind !== "note" && item.storage_path) {
    const btn = document.createElement("button");
    btn.className = "download";
    btn.textContent = "Download";
    btn.onclick = () => onDownload(item, btn);
    actions.append(btn);
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
