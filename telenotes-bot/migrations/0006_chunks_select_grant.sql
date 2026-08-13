-- 0004_semantic_search.sql revoked all privileges on `chunks` from both
-- `anon` and `authenticated`, then relied on the SECURITY INVOKER RPCs
-- (related_items) to scope reads to the caller. But a SECURITY INVOKER
-- function runs with the *caller's* privileges -- it still needs its own
-- table-level grant, the same as items/users get in 0001_init.sql. Without
-- it, related_items() fails outright with "permission denied for table
-- chunks" before RLS ever gets a chance to filter rows.
--
-- And RLS being enabled with zero policies means "deny every row" even once
-- the grant exists, so this also adds the `_select_own` policy every other
-- user-scoped table has.

grant select on chunks to authenticated;

drop policy if exists chunks_select_own on chunks;
create policy chunks_select_own on chunks
  for select to authenticated
  using (user_id in (select id from users where auth_user_id = auth.uid()));
