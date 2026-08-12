-- Semantic search needs a token too, for the same reason writes do:
-- embedding the query text requires the Voyage API key, a secret only the
-- bot holds, so the browser can't call Voyage (or pgvector's <=> operator
-- against an embedding it can't compute) directly. It reuses the existing
-- action_tokens/issue_write_token machinery rather than a parallel table --
-- a search is a one-shot capability exactly like a write, just read-only.

alter table action_tokens drop constraint if exists action_tokens_purpose_check;
alter table action_tokens add constraint action_tokens_purpose_check
  check (purpose in ('create_note','edit_item','upload_file','delete_item','semantic_search'));

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

  if purpose not in ('create_note','edit_item','upload_file','delete_item','semantic_search') then
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
