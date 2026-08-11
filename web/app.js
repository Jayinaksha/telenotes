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
import { fetchItems, fetchStats, fetchTags } from "./data.js";
import { flash, only, renderItems, renderStats, renderTags, show } from "./render.js";

const $ = (id) => document.getElementById(id);

if (SUPABASE_URL.includes("YOUR-PROJECT-REF")) {
  show($("boot-error"), "Edit web/config.js with your Supabase URL, anon key, and bot URL.");
  throw new Error("unconfigured");
}

const state = {
  kind: "", // "", "note", or "file"
  tag: null,
  query: "",
  items: [],
  editingId: null,
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
    $("signout").hidden = true;
    $("account").textContent = "";
    return only("view-auth");
  }

  $("signout").hidden = false;
  $("account").textContent = session.user.email;

  let linked;
  try {
    linked = await hasLinkedAccount();
  } catch (e) {
    return show($("boot-error"), e.message);
  }
  if (!linked) return only("view-link");

  only("view-main");
  await Promise.all([loadTags(), loadItems()]);
}

onAuthStateChange(() => route());

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

async function loadItems() {
  let rows;
  try {
    rows = await fetchItems({ query: state.query, kind: state.kind, tag: state.tag });
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

  render();
}

async function loadTags() {
  let data;
  try {
    data = await fetchTags();
  } catch {
    return;
  }
  renderTags(data, state.tag, (tag) => {
    state.tag = tag;
    loadTags();
    loadItems();
  });
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
  try {
    await api.addNote(text);
    input.value = "";
    flash(msg, "Saved.");
    await loadItems();
  } catch (err) {
    flash(msg, err.message, true);
  }
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
    flash(msg, "Uploaded.");
    await loadItems();
  } catch (err) {
    flash(msg, err.message, true);
  }
});

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

$("nav-stats").addEventListener("click", async () => {
  let tags, stats;
  try {
    [tags, stats] = await Promise.all([fetchTags(), fetchStats()]);
  } catch (e) {
    flash($("boot-error"), e.message, true);
    return;
  }
  renderStats(tags, stats.days, stats.storage);
  only("view-stats");
});

$("stats-back").addEventListener("click", () => only("view-main"));

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

route();
