// Client construction and auth. Every other module imports `supabase` from
// here rather than constructing its own client.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import { SUPABASE_URL, SUPABASE_ANON_KEY } from "./config.js";

export const supabase = createClient(SUPABASE_URL, SUPABASE_ANON_KEY);

export async function signIn(email, redirect) {
  return supabase.auth.signInWithOtp({
    email,
    options: { emailRedirectTo: redirect },
  });
}

export async function signOut() {
  await supabase.auth.signOut();
}

export async function getSession() {
  const {
    data: { session },
  } = await supabase.auth.getSession();
  return session;
}

export function onAuthStateChange(callback) {
  supabase.auth.onAuthStateChange(callback);
}

export async function linkTelegram(code) {
  return supabase.rpc("link_telegram", { code });
}

// RLS returns our own row and nothing else, so a hit here means this login
// is already bound to a Telegram account.
export async function hasLinkedAccount() {
  const { data: rows, error } = await supabase.from("users").select("id").limit(1);
  if (error) throw error;
  return Boolean(rows && rows.length);
}
