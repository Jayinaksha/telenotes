// Reads. Goes straight to Supabase via PostgREST/RPC, RLS-scoped -- the
// dashboard never asks the bot for anything but a download/write token.

import { supabase } from "./supabase.js";

export async function fetchItems({ query, kind, tag }) {
  if (query) {
    const { data, error } = await supabase.rpc("search_items", { q: query, lim: 200 });
    if (error) throw error;
    return data ?? [];
  }

  let q = supabase.from("items").select("*").order("created_at", { ascending: false }).limit(200);
  if (kind === "note") q = q.eq("kind", "note");
  else if (kind === "file") q = q.neq("kind", "note");
  if (tag) q = q.contains("tags", [tag]);

  const { data, error } = await q;
  if (error) throw error;
  return data ?? [];
}

export async function fetchTags() {
  const { data, error } = await supabase.rpc("tag_counts");
  if (error) throw error;
  return data ?? [];
}

export async function fetchStats() {
  const [{ data: days, error: daysError }, { data: storage, error: storageError }] =
    await Promise.all([
      supabase.rpc("items_per_day", { days: 30 }),
      supabase.rpc("storage_breakdown"),
    ]);
  if (daysError) throw daysError;
  if (storageError) throw storageError;
  return { days: days ?? [], storage: storage ?? [] };
}
