-- What a launch needs, and the server could not say.
--
-- A profile's fingerprint is derived from (persona, seed) plus a context: the
-- timezone it claims and the languages it sends. Those two had no columns, so
-- every team profile would have fallen back to UTC and en-US — while going out
-- through a German residential exit. That contradiction is the cheapest
-- detection signal there is, and avoiding it is the entire reason the persona
-- system exists.

ALTER TABLE profiles ADD COLUMN timezone  TEXT;
ALTER TABLE profiles ADD COLUMN languages TEXT[] NOT NULL DEFAULT '{}';

-- The seed is eight bytes. Enforced here because the three components that
-- touch it each had their own idea — BYTEA here, i64 in the local store, u64 at
-- the point of use — and a seed that round-trips differently is not a small
-- inconsistency. It is a different machine on every launch, for an account that
-- spent months building one. See `persona::seed` for the pinned encoding.
--
-- Existing rows are repaired first. Adding the constraint alone made the
-- migration fail on the first database it met, which is a server that will not
-- start — and "your data is inconsistent" delivered as a refusal to boot helps
-- nobody at the hour it usually arrives.
--
-- The repair is a hash of the old value rather than fresh randomness: it is
-- deterministic, so re-running gives the same answer, and it keeps two profiles
-- that had different seeds from ending up sharing one. Any row this touches was
-- never launchable — there was no endpoint that could create a profile until
-- this migration's release, so every such row was written by hand.
UPDATE profiles
   SET fp_seed = substring(sha256(fp_seed) from 1 for 8)
 WHERE octet_length(fp_seed) <> 8;

ALTER TABLE profiles ADD CONSTRAINT fp_seed_is_eight_bytes
    CHECK (octet_length(fp_seed) = 8);

-- The bundle key is not wrapped under the organisation key.
--
-- It is wrapped under a per-profile subkey derived from it, so that the agent —
-- the process that runs a browser full of other people's JavaScript — can be
-- handed enough to open the one profile it was asked to launch and nothing
-- more. A compromised agent leaks one profile, not the organisation.
COMMENT ON COLUMN bundles.wrapped_dek IS
    'Per-bundle DEK, wrapped under the profile subkey derived from the ORK.';
COMMENT ON COLUMN proxies.wrapped_dek IS
    'Per-proxy DEK, wrapped under the ORK. Opened by the desktop shell only.';
