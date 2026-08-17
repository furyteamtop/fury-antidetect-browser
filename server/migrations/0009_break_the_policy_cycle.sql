-- 0008 made two policies ask each other questions forever.
--
--   SQL function "user_holds_share" statement 1
--   SQL function "user_holds_share" statement 1
--   ... (a hundred more)
--   ERROR: stack depth limit exceeded
--
-- The cycle, in full:
--
--   policy on profiles        calls user_holds_share(id)
--   user_holds_share          reads profile_grants
--   policy on profile_grants  reads profiles          <-- back to the start
--
-- SECURITY DEFINER does not help and was never going to. It stops a function
-- recursing into a policy on ITS OWN table, which is what 0006 used it for.
-- This is two tables pointing at each other, and no amount of marking one
-- function breaks a loop that runs through both.
--
-- Found by the tests added with 0008 rather than by reading it, which is the
-- only reason it was found before anybody's profile list stopped loading: every
-- query against `profiles` fails this way once a single share exists, so the
-- feature would have broken the page it was written for.
--
-- THE FIX is to give profile_grants its own answer to "which organisation is
-- this?" so its policy never has to ask profiles. One denormalised column, and
-- the loop has nowhere to close.

ALTER TABLE profile_grants ADD COLUMN org_id UUID REFERENCES organizations(id) ON DELETE CASCADE;

-- Existing rows: every grant made before this migration was in-organisation by
-- construction, because nothing else was possible.
UPDATE profile_grants g
   SET org_id = p.org_id
  FROM profiles p
 WHERE p.id = g.profile_id AND g.org_id IS NULL;

-- Kept in step by the database rather than by every caller remembering. A grant
-- whose org_id disagreed with its profile's would be a grant the policy sends
-- to the wrong organisation, and that is not a mistake to leave to application
-- code.
--
-- SECURITY DEFINER, and this one genuinely needs it: the trigger reads
-- `profiles` while inserting into profile_grants, and doing that as the caller
-- would walk into the policy on profiles — which is the loop this migration
-- exists to remove.
CREATE OR REPLACE FUNCTION profile_grant_org() RETURNS TRIGGER
LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
    SELECT p.org_id INTO NEW.org_id FROM profiles p WHERE p.id = NEW.profile_id;
    IF NEW.org_id IS NULL THEN
        RAISE EXCEPTION 'no such profile: %', NEW.profile_id;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS profile_grant_org ON profile_grants;
CREATE TRIGGER profile_grant_org
    BEFORE INSERT OR UPDATE OF profile_id ON profile_grants
    FOR EACH ROW EXECUTE FUNCTION profile_grant_org();

ALTER TABLE profile_grants ALTER COLUMN org_id SET NOT NULL;

-- The policy that closed the loop, rewritten to answer from its own row.
--
-- Same meaning as 0008: the owning organisation sees every grant on its
-- profiles, and a grantee sees only the row addressed to them. What changed is
-- that neither branch reads `profiles`.
DROP POLICY IF EXISTS org_isolation ON profile_grants;
CREATE POLICY org_isolation ON profile_grants
    USING (user_in_org(org_id) OR user_id = current_app_user());

-- user_holds_share is unchanged and now terminates: it reads profile_grants,
-- whose policy reads org_members through user_in_org, which is SECURITY
-- DEFINER and reads nothing that leads back here.
