// Edit these three values, then commit. All of them are safe to publish.
//
// The anon key is *designed* to be public: it identifies the project, not
// you. Row Level Security is what actually protects your notes, which is why
// migrations/0001_init.sql revokes everything from the logged-out `anon` role.
// Never put the service_role key here -- that one bypasses RLS entirely.
export const SUPABASE_URL = "https://lspccfaedlmuervgznrd.supabase.co";
export const SUPABASE_ANON_KEY = "sb_publishable_TISj1MS0l7EihRw_gjHvpA_rpMB7z7g";

// Your deployed bot service, with no trailing slash. Used to redeem
// download tokens and to reach the write API (add/edit/upload/delete).
export const BOT_URL = "https://telenotes-bot.onrender.com";
