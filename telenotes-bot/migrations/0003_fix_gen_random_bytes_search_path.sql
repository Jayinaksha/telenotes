-- Fixes "function gen_random_bytes(integer) does not exist" from
-- issue_download_token and issue_write_token. Both SECURITY DEFINER
-- functions locked their search_path to `public` alone, but Supabase
-- pre-provisions pgcrypto into the `extensions` schema on most projects --
-- 0001_init.sql's `create extension if not exists pgcrypto` was a no-op
-- against that existing copy, so gen_random_bytes never actually lived in
-- public. Widening search_path to include `extensions` fixes both without
-- touching any other behavior. create or replace is idempotent, so this is
-- safe to run even if you already have these functions.

create or replace function issue_download_token(item_id uuid)
returns text
language plpgsql
security definer
set search_path = public, extensions
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

create or replace function issue_write_token(purpose text, item_id uuid default null)
returns text
language plpgsql
security definer
set search_path = public, extensions
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
