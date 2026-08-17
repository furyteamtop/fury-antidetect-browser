-- Give one profile to one person, across organisations.
--
-- WHAT THIS IS FOR. The team model is an organisation: everybody in it can
-- reach everything the projects let them, and nobody outside can reach
-- anything. That is right for a team and wrong for the thing people actually
-- asked for first -- "I want to hand this profile to that person" -- which in
-- every competing product is a per-profile share and not a membership.
--
-- WHAT IS ALREADY HERE. profile_grants, from 0001: a per-profile permission set
-- with an expiry. It has only ever been used inside one organisation, because
-- row-level security stops the grantee's queries before the grant is consulted.
-- So this migration adds no new idea; it removes the assumption that both ends
-- of a grant are in the same organisation.
--
-- WHAT IT COSTS, said plainly. The isolation this server rests on is
-- `user_in_org`, one condition on every table. Sharing widens it to
-- "in the org, OR holding a live grant to this row". A widened boundary is a
-- weaker boundary, and the only honest defence is that the widening is narrow:
-- one profile, one user, revocable, and it grants nothing about the
-- organisation the profile lives in -- not its other profiles, not its
-- projects, not its proxies except the one this profile goes out through.
--
-- WHAT REVOCATION DOES AND DOES NOT DO. Deleting the grant closes future
-- access. It does not reach the cookies the grantee already opened, and nothing
-- can: they were on that person's machine, in a browser, by design. This is
-- true of every product with this feature and it is worth writing down here
-- rather than implying otherwise in an interface.

-- ---------------------------------------------------------------------------
-- The keys the grantee cannot derive
-- ---------------------------------------------------------------------------
--
-- A member opens a profile's bundle with a subkey derived from the ORGANISATION
-- key. Someone outside has no such key and never will -- that is the point of
-- the organisation key.
--
-- So the share carries the keys, each sealed to the grantee's own public key
-- (X25519, shared-rs/src/keys.rs). The server stores two opaque blobs it cannot
-- open, which is the same thing it already does with wrapped_ork.
--
-- wrapped_proxy_key is not an afterthought and not optional in practice. A
-- profile handed over without its proxy goes out through the receiver's own
-- address, which for this product is not a degraded experience but the exact
-- failure it exists to prevent: an account that has only ever been seen from
-- one country appearing from another. NULL is allowed because a profile with no
-- proxy has no key to send.
ALTER TABLE profile_grants
    ADD COLUMN wrapped_key       BYTEA,
    ADD COLUMN wrapped_proxy_key BYTEA;

COMMENT ON COLUMN profile_grants.wrapped_key IS
    'The profile bundle key sealed to the grantee''s public key. NULL for a grant inside the owner''s own organisation, where the key is derivable.';
COMMENT ON COLUMN profile_grants.wrapped_proxy_key IS
    'The profile proxy''s data key, sealed the same way. NULL when the profile has no proxy.';

-- ---------------------------------------------------------------------------
-- Seeing across the boundary
-- ---------------------------------------------------------------------------

-- SECURITY DEFINER for the same reason user_in_org is: a policy that reads a
-- table which itself has a policy is a policy that can recurse. Owning the
-- function puts it outside that loop by construction rather than by anyone
-- remembering the rule.
--
-- The grant must be LIVE. An expired one is not a smaller permission, it is
-- none, and leaving that check to the application would mean a query that
-- forgot it saw rows it should not.
CREATE OR REPLACE FUNCTION user_holds_share(target_profile UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE SECURITY DEFINER AS $$
    SELECT EXISTS (
        SELECT 1 FROM profile_grants g
        WHERE g.profile_id = target_profile
          AND g.user_id = current_app_user()
          AND g.wrapped_key IS NOT NULL
          AND (g.expires_at IS NULL OR g.expires_at > now())
    )
$$;

-- The profile itself.
DROP POLICY IF EXISTS org_isolation ON profiles;
CREATE POLICY org_isolation ON profiles
    USING (user_in_org(org_id) OR user_holds_share(id));

-- Its bundles. Without this the grantee sees a profile it cannot open, which is
-- a worse outcome than not seeing it: the interface offers a launch that fails.
DROP POLICY IF EXISTS org_isolation ON bundles;
CREATE POLICY org_isolation ON bundles
    USING (EXISTS (SELECT 1 FROM profiles p
                   WHERE p.id = bundles.profile_id
                     AND (user_in_org(p.org_id) OR user_holds_share(p.id))));

-- Its lock. The lock is what stops two people opening one profile at once, and
-- a shared profile is exactly the case where two people might.
DROP POLICY IF EXISTS org_isolation ON profile_locks;
CREATE POLICY org_isolation ON profile_locks
    USING (EXISTS (SELECT 1 FROM profiles p
                   WHERE p.id = profile_locks.profile_id
                     AND (user_in_org(p.org_id) OR user_holds_share(p.id))));

-- The grant row, so the grantee can read the keys addressed to them.
--
-- Deliberately narrower than the others: a member of the owning organisation
-- sees every grant on their profiles, and the grantee sees ONLY their own row.
-- Who else a profile was shared with is the owner's business.
DROP POLICY IF EXISTS org_isolation ON profile_grants;
CREATE POLICY org_isolation ON profile_grants
    USING (
        EXISTS (SELECT 1 FROM profiles f
                WHERE f.id = profile_grants.profile_id AND user_in_org(f.org_id))
        OR profile_grants.user_id = current_app_user()
    );

-- The one proxy this profile goes out through, and nothing else in that
-- organisation's proxy list.
DROP POLICY IF EXISTS org_isolation ON proxies;
CREATE POLICY org_isolation ON proxies
    USING (
        user_in_org(org_id)
        OR EXISTS (SELECT 1 FROM profiles p
                   WHERE p.proxy_id = proxies.id
                     AND p.deleted_at IS NULL
                     AND user_holds_share(p.id))
    );

-- Finding somebody by their exact address, to share with them.
--
-- users has no policy today and gains none here; what it gains is the index the
-- lookup needs. The lookup is by WHOLE address and never by prefix, so it
-- answers "does this person exist" and never "who is on this server" -- the
-- difference between checking an address somebody gave you and enumerating a
-- user list.
CREATE INDEX IF NOT EXISTS users_email_lower ON users (lower(email));

-- Who was given what, and by whom.
CREATE INDEX IF NOT EXISTS profile_grants_user ON profile_grants (user_id);
