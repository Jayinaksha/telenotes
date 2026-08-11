import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import { SUPABASE_URL, SUPABASE_ANON_KEY, BOT_URL } from "./config.js";

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

const $ = (id) => document.getElementById(id);

if (SUPABASE_URL.includes("YOUR-PROJECT-REF")) {
  show($("boot-error"), "Edit web/config.js with your Supabase URL, anon key, and bot URL.");
  throw new Error("unconfigured");
}

const supabase = createClient(SUPABASE_URL, SUPABASE_ANON_KEY);

const state = {
  kind: "",       // "", "note", or "file"
  tag: null,
  query: "",
  items: [],
};

function show(el, text) {
  if (text !== undefined) el.textContent = text;
  el.hidden = false;
}

function only(viewId) {
  for (const id of ["view-auth", "view-link", "view-main"]) {
    $(id).hidden = id !== viewId;
  }
}

function flash(el, text, bad = false) {
  el.textContent = text;
  el.classList.toggle("bad", bad);
  el.hidden = false;
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

$("form-signin").addEventListener("submit", async (e) => {
  e.preventDefault();
  const email = $("email").value.trim();
  // Come back to this exact page, minus any stale token fragment.
  const redirect = location.origin + location.pathname;
  const { error } = await supabase.auth.signInWithOtp({
    email,
    options: { emailRedirectTo: redirect },
  });
  flash($("auth-msg"),
    error ? error.message : `Check ${email} for a sign-in link.`,
    Boolean(error));
});

$("signout").addEventListener("click", async () => {
  await supabase.auth.signOut();
  location.reload();
});

$("form-link").addEventListener("submit", async (e) => {
  e.preventDefault();
  const code = $("linkcode").value.trim().toUpperCase();
  const { data, error } = await supabase.rpc("link_telegram", { code });

  if (error) return flash($("link-msg"), error.message, true);
  if (data === "ok") {
    flash($("link-msg"), "Connected.");
    return route();
  }
  flash($("link-msg"),
    data === "not_signed_in"
      ? "Session expired — sign in again."
      : "That code is invalid or has expired. Send /link again.",
    true);
});

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

async function route() {
  const { data: { session } } = await supabase.auth.getSession();

  if (!session) {
    $("signout").hidden = true;
    $("account").textContent = "";
    return only("view-auth");
  }

  $("signout").hidden = false;
  $("account").textContent = session.user.email;

  // RLS returns our own row and nothing else, so a hit here means this login
  // is already bound to a Telegram account.
  const { data: rows, error } = await supabase.from("users").select("id").limit(1);
  if (error) return show($("boot-error"), error.message);
  if (!rows || rows.length === 0) return only("view-link");

  only("view-main");
  await Promise.all([loadTags(), loadItems()]);
}

supabase.auth.onAuthStateChange(() => route());

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

async function loadItems() {
  let rows, error;

  if (state.query) {
    ({ data: rows, error } = await supabase.rpc("search_items", {
      q: state.query,
      lim: 200,
    }));
  } else {
    let q = supabase
      .from("items")
      .select("*")
      .order("created_at", { ascending: false })
      .limit(200);
    if (state.kind === "note") q = q.eq("kind", "note");
    else if (state.kind === "file") q = q.neq("kind", "note");
    if (state.tag) q = q.contains("tags", [state.tag]);
    ({ data: rows, error } = await q);
  }

  if (error) return show($("boot-error"), error.message);

  // Search runs server-side without the facet filters, so apply them here to
  // keep the two paths showing the same thing.
  state.items = (rows ?? []).filter((it) => {
    if (state.kind === "note" && it.kind !== "note") return false;
    if (state.kind === "file" && it.kind === "note") return false;
    if (state.tag && !(it.tags ?? []).includes(state.tag)) return false;
    return true;
  });

  render();
}

async function loadTags() {
  const { data, error } = await supabase.rpc("tag_counts");
  if (error) return;

  const box = $("tags");
  box.replaceChildren();

  const all = document.createElement("button");
  all.textContent = "All tags";
  all.classList.toggle("active", state.tag === null);
  all.onclick = () => { state.tag = null; loadTags(); loadItems(); };
  box.append(all);

  for (const { tag, n } of data ?? []) {
    const b = document.createElement("button");
    b.classList.toggle("active", state.tag === tag);
    b.append(`#${tag}`);
    const count = document.createElement("span");
    count.className = "count";
    count.textContent = n;
    b.append(count);
    b.onclick = () => { state.tag = state.tag === tag ? null : tag; loadTags(); loadItems(); };
    box.append(b);
  }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function render() {
  const box = $("results");
  box.replaceChildren();

  if (state.items.length === 0) {
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent = state.query
      ? `Nothing matches “${state.query}”.`
      : "Nothing here yet. Send your bot a message.";
    box.append(p);
    return;
  }

  for (const item of state.items) box.append(renderItem(item));
}

function renderItem(item) {
  const el = document.createElement("article");
  el.className = "item";

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
    year: "numeric", month: "short", day: "numeric",
  });
  head.append(date);
  el.append(head);

  // textContent throughout: note bodies are arbitrary user text and must
  // never be parsed as HTML.
  if (item.content_text) {
    const body = document.createElement("p");
    body.className = "item-body";
    body.textContent = item.content_text;
    el.append(body);
  }

  if (item.kind !== "note" && item.storage_path) {
    const btn = document.createElement("button");
    btn.className = "download";
    btn.textContent = "Download";
    btn.onclick = () => download(item, btn);
    el.append(btn);
  }

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

/// Swaps an item id for a short-lived token, then lets the bot service serve
/// the bytes. The browser never sees storage credentials.
async function download(item, btn) {
  btn.disabled = true;
  btn.textContent = "Preparing…";
  try {
    const { data: token, error } = await supabase.rpc("issue_download_token", {
      item_id: item.id,
    });
    if (error) throw error;
    window.open(`${BOT_URL}/d/${token}`, "_blank", "noopener");
    btn.textContent = "Download";
  } catch (e) {
    btn.textContent = "Failed — retry";
    console.error(e);
  } finally {
    btn.disabled = false;
  }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

let searchTimer;
$("search").addEventListener("input", (e) => {
  clearTimeout(searchTimer);
  const value = e.target.value.trim();
  searchTimer = setTimeout(() => { state.query = value; loadItems(); }, 250);
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
