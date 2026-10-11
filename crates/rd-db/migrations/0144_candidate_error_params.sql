-- RD-1250-03: the parameters a LinkGrabber row's message is translated with, as a JSON object
-- of strings (`{"detail": "ERROR: [youtube] ...: Video unavailable"}`), redacted like the text
-- beside it. `NULL` for a message without parameters and for every row written before; the
-- interface then shows `error`, the English text, as it did.
ALTER TABLE link_candidates ADD COLUMN error_params_json TEXT;
