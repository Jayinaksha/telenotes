// Edit these three values, then commit. All of them are safe to publish.
//
// The anon key is *designed* to be public: it identifies the project, not
// you. Row Level Security is what actually protects your notes, which is why
// migrations/0001_init.sql revokes everything from the logged-out `anon` role.
// Never put the service_role key here -- that one bypasses RLS entirely.
export const SUPABASE_URL = "https://YOUR-PROJECT-REF.supabase.co";
export const SUPABASE_ANON_KEY = "YOUR-ANON-PUBLIC-KEY";

// Your deployed bot service, with no trailing slash. Used only to redeem
// download tokens.
export const BOT_URL = "https://telenotes-bot.onrender.com";
