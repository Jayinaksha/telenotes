-- telenotes schema.
--
-- Run this once in the Supabase SQL editor (Project -> SQL Editor -> New
-- query -> paste -> Run). It is written to be re-runnable.
--
-- Two very different clients touch these tables:
--
--   * the bot, connecting as the `postgres` role over the pooler. It owns
--     these tables, so Row Level Security does not apply to it.
--   * the web UI, connecting through PostgREST as `anon` (logged out) or
--     `authenticated` (logged in). RLS is the ONLY thing standing between a
--     stranger and your notes there, because a GitHub Pages site is publicly
--     reachable even when its source repo is private.

create extension if not exists pgcrypto;

-- ---------------------------------------------------------------------------
-- Tables
-- ---------------------------------------------------------------------------

create table if not exists users (
  id                uuid primary key default gen_random_uuid(),
  telegram_chat_id  bigint unique not null,
  username          text,
  -- Set by link_telegram() once you've proved, from the web UI, that you own
  -- both this Telegram account and a Supabase Auth login.
  auth_user_id      uuid unique references auth.users(id) on delete set null,
  created_at        timestamptz not null default now()
);

create table if not exists items (
  id              uuid primary key default gen_random_uuid(),
  user_id         uuid not null references users(id) on delete cascade,
  kind            text not null check (kind in ('note','document','photo','voice','video','audio')),
  content_text    text,
  file_id         text,
  file_unique_id  text,
  -- Which store holds the bytes: 'github' or 'r2'. Null on rows written
  -- before the hybrid backend existed; those are all GitHub.
  backend         text check (backend in ('github','r2')),
  storage_path    text,
  file_name       text,
  mime_type       text,
  file_size       bigint,
  tags            text[] not null default '{}',
  created_at      timestamptz not null default now(),
  updated_at      timestamptz not null default now(),
  search_vector   tsvector generated always as (
    to_tsvector('english', coalesce(content_text,'') || ' ' || coalesce(file_name,''))
  ) stored
);

-- Short-lived codes for the /link flow. The bot issues one; the web UI
-- redeems it after you've signed in.
create table if not exists link_codes (
  code        text primary key,
  user_id     uuid not null references users(id) on delete cascade,
  expires_at  timestamptz not null,
  used_at     timestamptz
);

-- Short-lived capability tokens for downloading an attachment from the web
-- UI. This exists so the browser never needs R2 credentials and the bot never
-- needs to verify a Supabase JWT: the UI proves ownership to Postgres (where
-- RLS already applies), Postgres mints a token, and the bot trades that token
-- for the bytes.
create table if not exists download_tokens (
  token       text primary key,
  item_id     uuid not null references items(id) on delete cascade,
  expires_at  timestamptz not null
);

alter table items add column if not exists backend text;
alter table items add column if not exists updated_at timestamptz not null default now();
alter table users add column if not exists auth_user_id uuid unique references auth.users(id) on delete set null;

create index if not exists items_user_created_idx on items(user_id, created_at desc);
create index if not exists items_search_idx       on items using gin(search_vector);
create index if not exists items_tags_idx         on items using gin(tags);
create index if not exists link_codes_expiry_idx  on link_codes(expires_at);

-- ---------------------------------------------------------------------------
-- updated_at maintenance
-- ---------------------------------------------------------------------------

create or replace function set_updated_at() returns trigger
language plpgsql as $$
begin
  new.updated_at := now();
  return new;
end;
$$;

drop trigger if exists items_set_updated_at on items;
create trigger items_set_updated_at
  before update on items
  for each row execute function set_updated_at();

-- ---------------------------------------------------------------------------
-- Row Level Security
-- ---------------------------------------------------------------------------

alter table users           enable row level security;
alter table items           enable row level security;
alter table link_codes      enable row level security;
alter table download_tokens enable row level security;

-- Logged-out visitors get nothing at all.
revoke all on users, items, link_codes, download_tokens from anon;
-- Read-only for the web UI, deliberately. Deleting an item also has to delete
-- the blob behind it, and only the bot holds the storage credentials -- so
-- deletion goes through /delete in Telegram, which cleans up both. Granting
-- DELETE here would let the dashboard strand files in R2 forever.
grant select on users, items to authenticated;

drop policy if exists users_select_own on users;
create policy users_select_own on users
  for select to authenticated
  using (auth_user_id = auth.uid());

drop policy if exists items_select_own on items;
create policy items_select_own on items
  for select to authenticated
  using (user_id in (select id from users where auth_user_id = auth.uid()));

-- No policies on link_codes or download_tokens: nothing but the bot and the
-- SECURITY DEFINER functions below may touch them, so codes and tokens can't
-- be enumerated by a signed-in user.

-- ---------------------------------------------------------------------------
-- RPCs called by the web UI
-- ---------------------------------------------------------------------------

-- Redeems a /link code, binding this Telegram account to the caller's
-- Supabase Auth login. SECURITY DEFINER because the caller cannot read
-- link_codes directly; the function does its own validation.
create or replace function link_telegram(code text)
returns text
language plpgsql
security definer
set search_path = public
as $$
declare
  target_user uuid;
begin
  if auth.uid() is null then
    return 'not_signed_in';
  end if;

  select lc.user_id into target_user
  from link_codes lc
  where lc.code = link_telegram.code
    and lc.used_at is null
    and lc.expires_at > now();

  if target_user is null then
    return 'invalid_or_expired';
  end if;

  -- One Telegram account per login, and vice versa.
  update users set auth_user_id = null where auth_user_id = auth.uid();
  update users set auth_user_id = auth.uid() where id = target_user;
  update link_codes set used_at = now() where link_codes.code = link_telegram.code;

  return 'ok';
end;
$$;

revoke all on function link_telegram(text) from public, anon;
grant execute on function link_telegram(text) to authenticated;

-- Full-text search. SECURITY INVOKER, so RLS still scopes results to the
-- caller's own rows.
create or replace function search_items(q text, lim int default 50)
returns setof items
language sql
stable
security invoker
set search_path = public
as $$
  select *
  from items
  where search_vector @@ plainto_tsquery('english', q)
  order by ts_rank(search_vector, plainto_tsquery('english', q)) desc,
           created_at desc
  limit least(coalesce(lim, 50), 200);
$$;

grant execute on function search_items(text, int) to authenticated;

-- Tag counts for the sidebar.
create or replace function tag_counts()
returns table (tag text, n bigint)
language sql
stable
security invoker
set search_path = public
as $$
  select t.tag, count(*) as n
  from items i, unnest(i.tags) as t(tag)
  group by t.tag
  order by n desc, t.tag asc;
$$;

grant execute on function tag_counts() to authenticated;

-- Mints a download token for an item you own. SECURITY DEFINER so it can
-- write to download_tokens, but it re-checks ownership against auth.uid()
-- rather than trusting the caller's item_id.
create or replace function issue_download_token(item_id uuid)
returns text
language plpgsql
security definer
set search_path = public
as $$
declare
  new_token text;
begin
  if auth.uid() is null then
    raise exception 'not signed in';
  end if;

  if not exists (
    select 1
    from items i
    join users u on u.id = i.user_id
    where i.id = issue_download_token.item_id
      and u.auth_user_id = auth.uid()
  ) then
    raise exception 'not found';
  end if;

  new_token := encode(gen_random_bytes(24), 'hex');
  insert into download_tokens (token, item_id, expires_at)
  values (new_token, issue_download_token.item_id, now() + interval '5 minutes');

  return new_token;
end;
$$;

revoke all on function issue_download_token(uuid) from public, anon;
grant execute on function issue_download_token(uuid) to authenticated;
