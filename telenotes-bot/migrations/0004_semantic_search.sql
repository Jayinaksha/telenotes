-- Semantic search: chunk every note/file's text, embed each chunk with
-- Voyage AI, and let pgvector find what's related by meaning instead of
-- keyword. Chunks are a separate table (not a column on items) because one
-- item -- especially a chunked PDF -- can have many embedded pieces, each
-- individually searchable so results can point at the paragraph that
-- matched, not just "this paper somewhere".

create extension if not exists vector;

create table if not exists chunks (
  id          uuid primary key default gen_random_uuid(),
  item_id     uuid not null references items(id) on delete cascade,
  user_id     uuid not null references users(id) on delete cascade,
  chunk_index int not null,
  content     text not null,
  -- 1024 dims: Voyage's flexible-dimension models (voyage-3.5, voyage-4,
  -- ...) default to 1024 and support requesting it explicitly via
  -- output_dimension -- the bot always passes that explicitly so this column
  -- size is never a guess at request time, only a guess about the model
  -- family's defaults today.
  embedding   vector(1024) not null,
  created_at  timestamptz not null default now(),
  unique (item_id, chunk_index)
);

-- HNSW over cosine distance: the bot's own similarity queries (both
-- related_items and semantic_search below) use <=> throughout, so the index
-- op class has to match or it's never used.
create index if not exists chunks_embedding_idx
  on chunks using hnsw (embedding vector_cosine_ops);

create index if not exists chunks_item_id_idx on chunks(item_id);

alter table chunks enable row level security;
revoke all on chunks from anon, authenticated;
-- No policies: only the bot (postgres role, RLS-exempt) writes chunks.
-- Reads go through the SECURITY INVOKER RPCs below, which re-derive the
-- caller's own user_id from auth.uid() rather than trusting a client-
-- supplied one -- the same discipline tag_counts() and search_items()
-- already use in 0001_init.sql.

-- ---------------------------------------------------------------------------
-- RPCs
-- ---------------------------------------------------------------------------

-- Items whose chunks are semantically closest to the given item's own
-- chunks, using each item's nearest single chunk-to-chunk distance (not a
-- centroid average) so a long paper that's only related to another paper in
-- one section still surfaces it.
create or replace function related_items(item_id uuid, match_count int default 8)
returns table (id uuid, kind text, content_text text, file_name text, mime_type text, storage_path text, tags text[], created_at timestamptz, distance float8)
language sql stable security invoker set search_path = public as $$
  with caller as (
    select u.id as user_id from users u where u.auth_user_id = auth.uid()
  ),
  target_chunks as (
    select c.embedding
    from chunks c
    join caller on c.user_id = caller.user_id
    where c.item_id = related_items.item_id
  )
  select i.id, i.kind, i.content_text, i.file_name, i.mime_type, i.storage_path, i.tags, i.created_at,
         min(c.embedding <=> t.embedding) as distance
  from chunks c
  join caller on c.user_id = caller.user_id
  join items i on i.id = c.item_id
  cross join target_chunks t
  where c.item_id <> related_items.item_id
  group by i.id, i.kind, i.content_text, i.file_name, i.mime_type, i.storage_path, i.tags, i.created_at
  order by distance asc
  limit least(coalesce(match_count, 8), 50);
$$;
grant execute on function related_items(uuid, int) to authenticated;

-- Free-text semantic search is deliberately not an RPC here: embedding the
-- query string needs the Voyage API key (a secret only the bot holds), so
-- that step -- and the pgvector query that follows it -- both happen
-- bot-side in POST /api/semantic-search, using the same claimed-token user
-- scoping every other write endpoint uses. See telenotes-bot/src/db.rs.
