import { SUPABASE_URL } from "./config.js";
import {
  getSession,
  hasLinkedAccount,
  linkTelegram,
  onAuthStateChange,
  signIn,
  signOut,
} from "./supabase.js";
import * as api from "./api.js";
import { renderNoteHtml } from "./markdown.js";
import { fetchItems, fetchStats, fetchTags } from "./data.js";
import {
  flash,
  renderItems,
  renderStats,
  renderTagFilter,
  renderTagList,
  show,
  showEntry,
  showPane,
} from "./render.js";

const $ = (id) => document.getElementById(id);

if (SUPABASE_URL.includes("YOUR-PROJECT-REF")) {
  show($("boot-error"), "Edit web/config.js with your Supabase URL, anon key, and bot URL.");
  throw new Error("unconfigured");
}

const state = {
  pane: "write",
  kind: "", // "", "note", or "file"
  tag: null,
  query: "",
  items: [],
  editingId: null,
  semantic: false,
};

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

$("form-signin").addEventListener("submit", async (e) => {
  e.preventDefault();
  const email = $("email").value.trim();
  // Come back to this exact page, minus any stale token fragment.
  const redirect = location.origin + location.pathname;
  const { error } = await signIn(email, redirect);
  flash($("auth-msg"), error ? error.message : `Check ${email} for a sign-in link.`, Boolean(error));
});

$("signout").addEventListener("click", async () => {
  await signOut();
  location.reload();
});

$("form-link").addEventListener("submit", async (e) => {
  e.preventDefault();
  const code = $("linkcode").value.trim().toUpperCase();
  const { data, error } = await linkTelegram(code);

  if (error) return flash($("link-msg"), error.message, true);
  if (data === "ok") {
    flash($("link-msg"), "Connected.");
    return route();
  }
  flash(
    $("link-msg"),
    data === "not_signed_in"
      ? "Session expired — sign in again."
      : "That code is invalid or has expired. Send /link again.",
    true,
  );
});

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

async function route() {
  const session = await getSession();

  if (!session) {
    $("account").textContent = "";
    return showEntry("view-auth");
  }

  $("account").textContent = session.user.email;

  let linked;
  try {
    linked = await hasLinkedAccount();
  } catch (e) {
    return show($("boot-error"), e.message);
  }
  if (!linked) return showEntry("view-link");

  showEntry("shell");
  switchPane(state.pane);
}

onAuthStateChange(() => route());

// ---------------------------------------------------------------------------
// Panes
// ---------------------------------------------------------------------------

async function switchPane(name) {
  state.pane = name;
  showPane(name);
  if (name === "browse") await loadItems();
  else if (name === "tags") await loadTagsPane();
  else if (name === "stats") await loadStatsPane();
}

for (const tab of document.querySelectorAll(".navtab")) {
  tab.addEventListener("click", () => switchPane(tab.dataset.view));
}

// ---------------------------------------------------------------------------
// Sidebar collapse
// ---------------------------------------------------------------------------

const COLLAPSE_KEY = "telenotes:sidebar-collapsed";

function setSidebarCollapsed(collapsed) {
  $("shell").classList.toggle("collapsed", collapsed);
  $("collapse-sidebar").setAttribute("aria-label", collapsed ? "Expand sidebar" : "Collapse sidebar");
  localStorage.setItem(COLLAPSE_KEY, collapsed ? "1" : "0");
}

setSidebarCollapsed(localStorage.getItem(COLLAPSE_KEY) === "1");

$("collapse-sidebar").addEventListener("click", () => {
  setSidebarCollapsed(!$("shell").classList.contains("collapsed"));
});

async function loadTagsPane() {
  let data;
  try {
    data = await fetchTags();
  } catch (e) {
    return flash($("boot-error"), e.message, true);
  }
  renderTagList(data, (tag) => {
    state.tag = tag;
    switchPane("browse");
  });
}

async function loadStatsPane() {
  let tags, stats;
  try {
    [tags, stats] = await Promise.all([fetchTags(), fetchStats()]);
  } catch (e) {
    return flash($("boot-error"), e.message, true);
  }
  renderStats(tags, stats.days, stats.storage);
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

async function loadItems() {
  let rows;
  try {
    rows =
      state.semantic && state.query
        ? await api.semanticSearch(state.query)
        : await fetchItems({ query: state.query, kind: state.kind, tag: state.tag });
  } catch (e) {
    return show($("boot-error"), e.message);
  }

  // Search runs server-side without the facet filters, so apply them here to
  // keep the two paths showing the same thing.
  state.items = rows.filter((it) => {
    if (state.kind === "note" && it.kind !== "note") return false;
    if (state.kind === "file" && it.kind === "note") return false;
    if (state.tag && !(it.tags ?? []).includes(state.tag)) return false;
    return true;
  });

  renderTagFilter(state.tag, () => {
    state.tag = null;
    loadItems();
  });
  render();
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function render() {
  renderItems(state.items, state.query, state.editingId, {
    onDownload,
    onEdit: (item) => {
      state.editingId = item.id;
      render();
    },
    onCancelEdit: () => {
      state.editingId = null;
      render();
    },
    onSaveEdit,
    onDelete,
    onRenderNote: (item) => renderNoteHtml(item.content_text),
    onResolveUrl: (item) => api.resolveUrl(item),
    onRelated: (item) => api.relatedItems(item.id),
  });
}

async function onDownload(item, btn) {
  btn.disabled = true;
  btn.textContent = "Preparing…";
  try {
    await api.download(item);
    btn.textContent = "Download";
  } catch (e) {
    btn.textContent = "Failed — retry";
    console.error(e);
  } finally {
    btn.disabled = false;
  }
}

async function onSaveEdit(item, patch) {
  try {
    await api.editItem(item.id, patch);
    state.editingId = null;
    await loadItems();
  } catch (e) {
    flash($("boot-error"), e.message, true);
  }
}

async function onDelete(item, btn) {
  if (!confirm(`Delete this ${item.kind}? This also removes the stored file.`)) return;
  btn.disabled = true;
  try {
    await api.deleteItem(item.id);
    await loadItems();
  } catch (e) {
    btn.disabled = false;
    flash($("boot-error"), e.message, true);
  }
}

// ---------------------------------------------------------------------------
// Add note / upload file
// ---------------------------------------------------------------------------

$("form-note").addEventListener("submit", async (e) => {
  e.preventDefault();
  const input = $("note-text");
  const text = input.value.trim();
  if (!text) return;
  const msg = $("note-msg");
  const btn = $("note-save");

  btn.disabled = true;
  btn.textContent = "Saving…";
  try {
    await api.addNote(text);
    input.value = "";
    input.style.height = "";
    msg.hidden = true;
    btn.textContent = "Saved";
    setTimeout(() => (btn.textContent = "Save note"), 1200);
  } catch (err) {
    btn.disabled = false;
    btn.textContent = "Save note";
    flash(msg, err.message, true);
  }
});

// The Save button only lights up once there's something worth saving. The
// textarea fills the screen via CSS (flex) and scrolls internally on its
// own -- no JS resizing needed.
$("note-text").addEventListener("input", (e) => {
  $("note-save").disabled = e.target.value.trim().length === 0;
  if (!$("note-msg").hidden) $("note-msg").hidden = true;
});

$("toggle-upload").addEventListener("click", () => {
  $("form-upload").hidden = !$("form-upload").hidden;
});

$("cancel-upload").addEventListener("click", () => {
  $("form-upload").hidden = true;
});

$("form-upload").addEventListener("submit", async (e) => {
  e.preventDefault();
  const fileInput = $("upload-file");
  const file = fileInput.files[0];
  const msg = $("upload-msg");
  if (!file) return flash(msg, "Choose a file first.", true);
  try {
    await api.uploadFile(file, $("upload-kind").value, $("upload-caption").value.trim());
    fileInput.value = "";
    $("upload-caption").value = "";
    $("form-upload").hidden = true;
    await loadItems();
  } catch (err) {
    flash(msg, err.message, true);
  }
});

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

let searchTimer;
$("search").addEventListener("input", (e) => {
  clearTimeout(searchTimer);
  const value = e.target.value.trim();
  searchTimer = setTimeout(() => {
    state.query = value;
    loadItems();
  }, 250);
});

for (const chip of document.querySelectorAll(".chip")) {
  chip.addEventListener("click", () => {
    for (const c of document.querySelectorAll(".chip")) c.classList.remove("active");
    chip.classList.add("active");
    state.kind = chip.dataset.kind;
    loadItems();
  });
}

$("toggle-semantic").addEventListener("click", () => {
  state.semantic = !state.semantic;
  const btn = $("toggle-semantic");
  btn.classList.toggle("active", state.semantic);
  btn.setAttribute("aria-pressed", String(state.semantic));
  $("search").placeholder = state.semantic
    ? "Search by meaning — e.g. \"papers about attention mechanisms\"…"
    : "Search notes, captions, filenames…";
  if (state.query) loadItems();
});

route();
