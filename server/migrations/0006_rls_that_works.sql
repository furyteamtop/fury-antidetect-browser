-- Make row-level security actually do something.
--
-- 0001 declared policies on six tables and they have been inert since. Two
-- separate reasons, both measured against a real PostgreSQL 16 rather than read
-- off the schema:
--
--   1. The app connects as the OWNER of every table. deploy/server-install.sh
--      creates role `fury` and runs `createdb -O fury`, so the migrations and
--      the server share one role, and PostgreSQL exempts a table's owner from
--      its own policies unless the table says FORCE. Measured: a connection as
--      `fury`, with app.user_id set to a user belonging to organisation A,
--      returned BOTH organisations' projects — and returned them with
--      app.user_id unset entirely.
--
--   2. Nothing ever set app.user_id. rbac_guard::bind_rls_user existed and had
--      no callers.
--
--  Fixing one without the other is worse than fixing neither: FORCE alone turns
--  every query into "no rows" the moment the setting is missing, and the
--  binding alone changes nothing while the owner exemption stands. This
--  migration does the first half; auth::Db does the second, and its tests fail
--  if either half is undone.
--
-- After FORCE, the same measurement gives 1, 1 and 0 — user A sees one project,
-- user B sees theirs, an unbound connection sees none — and an INSERT naming a
-- foreign org_id is refused rather than accepted.

-- ---------------------------------------------------------------------------
-- The functions the policies call
-- ---------------------------------------------------------------------------

-- SECURITY DEFINER so a policy cannot be made to recurse into another policy.
--
-- user_in_org() reads org_members. If org_members ever gains RLS — and a later
-- migration might reasonably want it to — a policy on projects would consult a
-- policy on org_members, which consults org_members again. As the owner of the
-- function, this runs outside that loop by construction rather than by anybody
-- remembering.
CREATE OR REPLACE FUNCTION user_in_org(target_org UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE SECURITY DEFINER AS $$
    SELECT EXISTS (
        SELECT 1 FROM org_members m
        WHERE m.org_id = target_org AND m.user_id = current_app_user()
    )
$$;

-- ---------------------------------------------------------------------------
-- The tables 0001 declared
-- ---------------------------------------------------------------------------

ALTER TABLE projects     FORCE ROW LEVEL SECURITY;
ALTER TABLE profiles     FORCE ROW LEVEL SECURITY;
ALTER TABLE proxies      FORCE ROW LEVEL SECURITY;
ALTER TABLE bundles      FORCE ROW LEVEL SECURITY;
ALTER TABLE credentials  FORCE ROW LEVEL SECURITY;
ALTER TABLE audit_events FORCE ROW LEVEL SECURITY;

-- 0001's policies carry USING and no WITH CHECK. For SELECT, UPDATE and DELETE
-- that is the whole story; for INSERT, PostgreSQL falls back to USING, so an
-- insert naming a foreign org is already refused. Stated here because relying
-- on a fallback silently is how the next person removes it.

-- ---------------------------------------------------------------------------
-- The tables 0001 missed
-- ---------------------------------------------------------------------------
--
-- Four tables hold organisation data and had no policy at all. A grant is who
-- may open which account; a lock is who has one open right now; an autofill
-- token is a credential in all but name. Leaving them uncovered would have made
-- the second line of defence cover the profiles and not the keys to them.

ALTER TABLE project_grants  ENABLE ROW LEVEL SECURITY;
ALTER TABLE project_grants  FORCE  ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON project_grants
    USING (EXISTS (SELECT 1 FROM projects p
                   WHERE p.id = project_grants.project_id AND user_in_org(p.org_id)));

ALTER TABLE profile_grants  ENABLE ROW LEVEL SECURITY;
ALTER TABLE profile_grants  FORCE  ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON profile_grants
    USING (EXISTS (SELECT 1 FROM profiles f
                   WHERE f.id = profile_grants.profile_id AND user_in_org(f.org_id)));

ALTER TABLE profile_locks   ENABLE ROW LEVEL SECURITY;
ALTER TABLE profile_locks   FORCE  ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON profile_locks
    USING (EXISTS (SELECT 1 FROM profiles f
                   WHERE f.id = profile_locks.profile_id AND user_in_org(f.org_id)));

ALTER TABLE autofill_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE autofill_tokens FORCE  ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON autofill_tokens
    USING (EXISTS (SELECT 1 FROM profiles f
                   WHERE f.id = autofill_tokens.profile_id AND user_in_org(f.org_id)));

-- ---------------------------------------------------------------------------
-- The tables deliberately left uncovered, and why
-- ---------------------------------------------------------------------------
--
--   users, sessions   The authentication path reads both BEFORE it knows who
--                     the caller is — that is what it is for. A policy keyed on
--                     current_app_user() would deny the query that establishes
--                     current_app_user(), and every login would fail.
--
--   org_members       user_in_org() reads it. SECURITY DEFINER above makes a
--                     policy here survivable, but the table is also read by the
--                     session lookup for the same reason as above.
--
--   organizations     Signup creates an organisation before any member exists,
--                     so no policy phrased in terms of membership can permit
--                     the insert. The row holds a name and a rotation counter;
--                     the things worth protecting hang off it and are covered.
--
--   enrollments       An invitation is peeked at by someone who is not yet
--                     anybody — that is the whole point of an invitation code.
--
-- Each of these is reachable only through a handler that checks, and the
-- handlers that read them are the auth path rather than the data path.

-- ---------------------------------------------------------------------------
-- Audit stays append-only
-- ---------------------------------------------------------------------------
--
-- 0001 asked for this in a comment addressed to a human, which is a thing that
-- does not happen. It is a statement now.
--
-- FROM CURRENT_USER, not FROM PUBLIC, and the difference is the whole thing.
-- The app owns this table, and an owner's privileges live in the ACL as an
-- explicit grant to itself — `{fury=arwdDxt/fury}` — which a revoke aimed at
-- PUBLIC does not touch. Measured: with the PUBLIC form in place, an UPDATE
-- from the application rewrote an audit row and reported success.
--
-- An owner revoking from itself is allowed, and leaves `{fury=arDxt/fury}`:
-- insert and select stay, update and delete are gone. It can be granted back,
-- which is as it should be — the point is that undoing this is a deliberate
-- statement somebody has to write, not the default.
REVOKE UPDATE, DELETE ON audit_events FROM CURRENT_USER;
