// Bot write calls. Same two-step shape throughout: mint a short-lived,
// single-use capability token via the `issue_write_token` Postgres RPC
// (which re-checks ownership itself), then hand it to the bot's `/api/*`
// endpoint that actually performs the write. The browser never gets storage
// credentials -- only the bot holds those.

import { supabase } from "./supabase.js";
import { BOT_URL } from "./config.js";

async function writeToken(purpose, itemId = null) {
  const { data: token, error } = await supabase.rpc("issue_write_token", {
    purpose,
    item_id: itemId,
  });
  if (error) throw error;
  return token;
}

async function ok(resp) {
  if (!resp.ok) throw new Error(await resp.text());
  return resp.json();
}

export async function addNote(text) {
  const token = await writeToken("create_note");
  const resp = await fetch(`${BOT_URL}/api/notes`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token, text }),
  });
  return ok(resp);
}

export async function uploadFile(file, kind, caption) {
  const token = await writeToken("upload_file");
  const form = new FormData();
  form.append("token", token);
  form.append("kind", kind);
  form.append("file", file, file.name);
  if (caption) form.append("caption", caption);
  const resp = await fetch(`${BOT_URL}/api/files`, { method: "POST", body: form });
  return ok(resp);
}

export async function editItem(itemId, patch) {
  const token = await writeToken("edit_item", itemId);
  const resp = await fetch(`${BOT_URL}/api/items/${itemId}/edit`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token, ...patch }),
  });
  return ok(resp);
}

export async function deleteItem(itemId) {
  const token = await writeToken("delete_item", itemId);
  const resp = await fetch(
    `${BOT_URL}/api/items/${itemId}?token=${encodeURIComponent(token)}`,
    { method: "DELETE" },
  );
  return ok(resp);
}

/// Swaps an item id for a short-lived download token and returns the URL it
/// resolves to. The browser never sees storage credentials -- only this
/// signed, single-purpose URL, minted fresh per call.
export async function resolveUrl(item) {
  const { data: token, error } = await supabase.rpc("issue_download_token", {
    item_id: item.id,
  });
  if (error) throw error;
  return `${BOT_URL}/d/${token}`;
}

export async function download(item) {
  window.open(await resolveUrl(item), "_blank", "noopener");
}
