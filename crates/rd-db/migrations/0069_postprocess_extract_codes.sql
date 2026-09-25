-- RD-108-08: extraction steps used to carry their stable code inside the message text.
--
-- The `code` column arrived with 0064, after the extraction steps were written (RD-107-11), so
-- those steps smuggled `extract.data_damaged` into `message` and the interface split it out
-- again. There is one wire format now, and the renderer that did the splitting is gone — which
-- would leave every row written before this release showing its raw code as prose. So the rows
-- are rewritten once: the leading token becomes the code, the remainder becomes the `detail`
-- parameter the catalogues interpolate, and what is left of the text stays the fallback.
UPDATE postprocess_steps
   SET code = CASE
                WHEN instr(message, ' ') > 0 THEN substr(message, 1, instr(message, ' ') - 1)
                ELSE message
              END,
       params_json = CASE
                WHEN length(trim(substr(message, instr(message, ' ') + 1))) > 0
                     AND instr(message, ' ') > 0
                  THEN json_object('detail', trim(substr(message, instr(message, ' ') + 1)))
                ELSE params_json
              END,
       message = CASE
                WHEN length(trim(substr(message, instr(message, ' ') + 1))) > 0
                     AND instr(message, ' ') > 0
                  THEN trim(substr(message, instr(message, ' ') + 1))
                ELSE NULL
              END
 WHERE code IS NULL
   AND message LIKE 'extract.%'
   -- Only the kinds the unpack and RAR-test jobs write. A migration cannot be taken back, and
   -- any other step whose text happens to start with "extract." is not one of these rows.
   AND kind IN ('extract_zip', 'extract_seven_zip', 'extract_rar', 'rar_test');
