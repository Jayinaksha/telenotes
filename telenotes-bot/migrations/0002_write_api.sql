-- Write API: lets the dashboard and TUI create, edit, upload to, and delete
-- items -- not just read them -- without ever handing the browser storage
-- credentials or a Supabase JWT the bot has to verify.
--
-- Same shape as 0001's `issue_download_token` / `download_tokens` pattern:
-- the caller proves ownership to Postgres (where RLS already applies),
-- Postgres mints a short-lived opaque token, and the bot trades that token
-- for an action. Run this once in the Supabase SQL editor after 0001. It is
-- written to be re-runnable.

-- ---------------------------------------------------------------------------
-- Notion bookkeeping
-- ---------------------------------------------------------------------------

-- Lets an edit or delete find the mirrored Notion page to update or archive.
-- Internal only -- the bot never includes this in its JSON responses.
alter table items add column if not exists notion_page_id text;

-- ---------------------------------------------------------------------------
-- Write-capability tokens
-- ---------------------------------------------------------------------------

-- Unlike download_tokens (harmless to reuse within the TTL -- a repeat
-- download is a no-op), writes must not be replayable. Single-use is
-- enforced by the bot with one atomic UPDATE ... SET used_at = now() ...
-- RETURNING on claim, not by anything in this schema.
create table if not exists action_tokens (
  token       text primary key,
  user_id     uuid not null references users(id) on delete cascade,
  purpose     text not null check (purpose in ('create_note','edit_item','upload_file','delete_item')),
  item_id     uuid references items(id) on delete cascade,
  expires_at  timestamptz not null,
  used_at     timestamptz,
  -- edit_item/delete_item act on an existing item and must name it;
  -- create_note/upload_file create a new item, so there's nothing to bind to
  -- yet.
  check ((purpose in ('edit_item','delete_item')) = (item_id is not null))
);

alter table action_tokens enable row level security;
revoke all on action_tokens from anon, authenticated;
-- No policies: only the bot (connecting as the unrestricted postgres role)
-- and issue_write_token() below may touch this table, exactly like
-- download_tokens -- so tokens can't be enumerated by a signed-in user.

create index if not exists action_tokens_expiry_idx on action_tokens(expires_at);

-- Mints a write-capability token. One generic function rather than one per
-- purpose: the ownership check only branches on whether item_id is present,
-- and issue_download_token's re-check-don't-trust-the-caller discipline
-- carries over unchanged.
create or replace function issue_write_token(purpose text, item_id uuid default null)
returns text
language plpgsql
security definer
set search_path = public
as $$
declare
  new_token text;
  caller uuid;
begin
  if auth.uid() is null then
    raise exception 'not signed in';
  end if;

  select id into caller from users where auth_user_id = auth.uid();
  if caller is null then
    raise exception 'no linked account';
  end if;

  if purpose not in ('create_note','edit_item','upload_file','delete_item') then
    raise exception 'invalid purpose';
  end if;

  if purpose in ('edit_item','delete_item') then
    if item_id is null then
      raise exception 'item_id required for %', purpose;
    end if;
    if not exists (
      select 1 from items where id = issue_write_token.item_id and user_id = caller
    ) then
      raise exception 'not found';
    end if;
  elsif item_id is not null then
    raise exception 'item_id must be null for %', purpose;
  end if;

  new_token := encode(gen_random_bytes(24), 'hex');
  insert into action_tokens (token, user_id, purpose, item_id, expires_at)
  values (new_token, caller, purpose, issue_write_token.item_id, now() + interval '5 minutes');

  return new_token;
end;
$$;

revoke all on function issue_write_token(text, uuid) from public, anon;
grant execute on function issue_write_token(text, uuid) to authenticated;

-- ---------------------------------------------------------------------------
-- Analytics
-- ---------------------------------------------------------------------------

-- Items captured per day, last N days (capped at a year). SECURITY INVOKER,
-- same trick tag_counts() uses: no explicit user_id filter needed because
-- items_select_own RLS already scopes the query to the caller.
create or replace function items_per_day(days int default 30)
returns table (day date, n bigint)
language sql
stable
security invoker
set search_path = public
as $$
  select created_at::date as day, count(*) as n
  from items
  where created_at >= now() - (least(coalesce(days, 30), 365) || ' days')::interval
  group by day
  order by day;
$$;

grant execute on function items_per_day(int) to authenticated;

-- Storage usage by kind and backend.
create or replace function storage_breakdown()
returns table (kind text, backend text, n bigint, bytes bigint)
language sql
stable
security invoker
set search_path = public
as $$
  select kind, coalesce(backend, 'github') as backend,
         count(*) as n, coalesce(sum(file_size), 0) as bytes
  from items
  group by kind, backend
  order by bytes desc;
$$;

grant execute on function storage_breakdown() to authenticated;
