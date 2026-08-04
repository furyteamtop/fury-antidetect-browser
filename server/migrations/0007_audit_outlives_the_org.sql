-- Let an organisation be deleted without letting its audit be rewritten.
--
-- 0006 made audit_events append-only by revoking UPDATE and DELETE from the
-- application role, which is right and which had a consequence nobody looked
-- for: an organisation became impossible to delete. Not merely awkward —
-- impossible, for everybody, including a superuser, because a foreign key's ON
-- DELETE CASCADE runs with the privileges of the referencing table's OWNER, and
-- the owner is exactly who was revoked.
--
--   DELETE FROM organizations WHERE ...
--   ERROR:  permission denied for table audit_events
--   CONTEXT: SQL statement "DELETE FROM ONLY public.audit_events WHERE ..."
--
-- Found by trying to remove a test organisation from a live server, which is
-- the only way this was ever going to be found: every test in the suite creates
-- organisations and none of them removes one.
--
-- It matters most on the deployment it was written for. With open sign-ups and
-- FURY_MAX_ORGS, every organisation ever created counts against the ceiling —
-- deliberately, so that delete-and-recreate cannot walk past it — and an
-- abandoned or abusive one that cannot be removed is a slot gone for good.
--
-- The fix is to decide what audit is FOR. It is the record of who did what, and
-- the moment it is most wanted is after somebody's access has been taken away.
-- An audit trail that disappears with the organisation it describes is an audit
-- trail that vanishes exactly when it is needed, so:
--
--   the organisation goes, the audit stays.
--
-- org_id becomes nullable and SET NULL on delete. The rows survive with nothing
-- to point at, which is what they are: a record of an organisation that no
-- longer exists. The policy from 0001 keys on user_in_org(org_id) and
-- user_in_org(NULL) is false, so orphaned rows are invisible to every
-- application user — readable only by somebody with database access, which is
-- the right audience for the history of a deleted tenant.

-- The grant that makes SET NULL possible without making audit rewritable.
--
-- SET NULL is an UPDATE, and 0006 revoked UPDATE outright — so the first
-- version of this migration swapped one impossibility for another: the delete
-- still failed, now on "permission denied ... UPDATE ONLY audit_events SET
-- org_id = NULL". Caught by the test below, which is why it is written the way
-- it is.
--
-- Column-level grants are the precise instrument. What append-only has to
-- protect is WHAT HAPPENED — action, detail, target, actor, the timestamp. A
-- pointer to an organisation that no longer exists is bookkeeping, not history,
-- and letting it be nulled costs nothing. So exactly two columns are writable
-- and every other one stays as it was written:
--
--   UPDATE audit_events SET action = 'something else'   -- still refused
--   UPDATE audit_events SET detail = '{}'               -- still refused
--   DELETE FROM audit_events                            -- still refused
--
-- and the cascade from a deleted organisation goes through.
GRANT UPDATE (org_id, actor_user_id) ON audit_events TO CURRENT_USER;

ALTER TABLE audit_events ALTER COLUMN org_id DROP NOT NULL;

ALTER TABLE audit_events DROP CONSTRAINT audit_events_org_id_fkey;
ALTER TABLE audit_events
    ADD CONSTRAINT audit_events_org_id_fkey
    FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE SET NULL;

-- Same reasoning one table over, and the same trap. A user who is deleted takes
-- their audit rows' actor with them, and actor_user_id was already nullable —
-- but the constraint had no ON DELETE action at all, so deleting a user failed
-- instead of cascading. NO ACTION was not a decision, it was the default.
ALTER TABLE audit_events DROP CONSTRAINT audit_events_actor_user_id_fkey;
ALTER TABLE audit_events
    ADD CONSTRAINT audit_events_actor_user_id_fkey
    FOREIGN KEY (actor_user_id) REFERENCES users(id) ON DELETE SET NULL;

-- And the one that blocked it from the other side: a project records who
-- created it, and deleting that person failed rather than forgetting them.
-- Deleting a user must not require unpicking everything they ever made.
ALTER TABLE projects DROP CONSTRAINT projects_created_by_fkey;
ALTER TABLE projects ALTER COLUMN created_by DROP NOT NULL;
ALTER TABLE projects
    ADD CONSTRAINT projects_created_by_fkey
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
