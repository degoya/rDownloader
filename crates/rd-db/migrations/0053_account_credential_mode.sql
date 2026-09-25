-- Which credential an account's single secret slot holds, for a provider that offers a
-- choice between signing in with a password and pasting a ready-made API key (ddownload).
-- NULL means the account predates the choice, and the provider's first declared mode
-- applies -- for ddownload that is `api_key`, i.e. exactly what such a row meant before.
ALTER TABLE accounts ADD COLUMN credential_mode TEXT;
