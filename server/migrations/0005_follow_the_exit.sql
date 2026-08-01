-- A profile follows its exit when it has no stored value, and that is the whole
-- rule.
--
-- `auto_timezone`, `auto_locale` and `auto_geo` have been in profiles since
-- 0001 and nothing has ever read or written one of them. They are not a feature
-- that was left unfinished, they are a second way to say something the value
-- already says: a NULL timezone means follow the exit, a stored one means use
-- it. Keeping both invites the two to disagree, and every existing row has the
-- flags TRUE while carrying a timezone that was deliberately set — so the first
-- code to honour them would have thrown those timezones away.
--
-- Dropping them rather than wiring them up. A column nothing reads is a claim
-- nobody is keeping.
ALTER TABLE profiles DROP COLUMN IF EXISTS auto_timezone;
ALTER TABLE profiles DROP COLUMN IF EXISTS auto_locale;
ALTER TABLE profiles DROP COLUMN IF EXISTS auto_geo;

-- And make the absence expressible. Both columns were written by an API that
-- refused an empty value, so no row can currently say "follow the exit" even
-- though the launch path now understands it.
--
-- languages is NOT NULL DEFAULT '{}' already, so an empty array is its way of
-- saying nothing; timezone is nullable already. Nothing to change in the shape
-- — only in the endpoints that refused to write it, which is a code change in
-- create_profile and update_profile rather than a schema one.
