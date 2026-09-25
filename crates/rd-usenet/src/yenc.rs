use anyhow::{Context, Result, bail};
use crc32fast::Hasher;

/// Metadata parsed from `=ybegin`, `=ypart` and `=yend` lines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YencMetadata {
    pub name: String,
    pub declared_size: u64,
    pub part_begin: Option<u64>,
    pub part_end: Option<u64>,
    pub expected_crc32: Option<u32>,
}

/// Decoded article plus verified metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedArticle {
    pub metadata: YencMetadata,
    pub data: Vec<u8>,
    pub crc32: u32,
}

/// Decodes one complete yEnc article and verifies size and CRC when present.
pub fn decode_yenc(article: &[u8]) -> Result<DecodedArticle> {
    let mut lines = article.split(|byte| *byte == b'\n').map(trim_cr);
    let begin = lines
        .find(|line| line.starts_with(b"=ybegin "))
        .context("missing =ybegin line")?;
    let begin_fields = parse_fields(begin)?;
    let name = begin_fields
        .iter()
        .find_map(|(key, value)| (*key == "name").then_some(*value))
        .context("missing yEnc name")?
        .to_owned();
    let declared_size = numeric(&begin_fields, "size")?;

    let mut part_begin = None;
    let mut part_end = None;
    let mut decoded = Vec::new();
    let mut expected_crc32 = None;
    let mut found_end = false;
    for line in lines {
        if line.starts_with(b"=ypart ") {
            let fields = parse_fields(line)?;
            part_begin = Some(numeric(&fields, "begin")?);
            part_end = Some(numeric(&fields, "end")?);
            continue;
        }
        if line.starts_with(b"=yend ") {
            let fields = parse_fields(line)?;
            // `pcrc32` is this part's checksum; `crc32` is the whole file's. Falling back to
            // `crc32` on a multipart article compared a part against the checksum of the
            // assembled file, so every article from a poster that emits only `crc32=` was
            // declared corrupt, burned through the backup servers and was left to PAR2.
            expected_crc32 = text(&fields, "pcrc32")
                .or_else(|| {
                    part_begin
                        .is_none()
                        .then(|| text(&fields, "crc32"))
                        .flatten()
                })
                .map(|value| u32::from_str_radix(value, 16).context("invalid yEnc CRC32"))
                .transpose()?;
            let encoded_size = numeric(&fields, "size")?;
            if encoded_size != decoded.len() as u64 {
                bail!(
                    "yEnc part size mismatch: expected {encoded_size}, got {}",
                    decoded.len()
                );
            }
            found_end = true;
            break;
        }
        decode_line(line, &mut decoded)?;
    }
    if !found_end {
        bail!("missing =yend line");
    }
    if part_begin.is_none() && declared_size != decoded.len() as u64 {
        bail!(
            "yEnc file size mismatch: expected {declared_size}, got {}",
            decoded.len()
        );
    }
    let mut hasher = Hasher::new();
    hasher.update(&decoded);
    let crc32 = hasher.finalize();
    if expected_crc32.is_some_and(|expected| expected != crc32) {
        bail!("yEnc CRC32 mismatch");
    }
    Ok(DecodedArticle {
        metadata: YencMetadata {
            name,
            declared_size,
            part_begin,
            part_end,
            expected_crc32,
        },
        data: decoded,
        crc32,
    })
}

fn decode_line(line: &[u8], output: &mut Vec<u8>) -> Result<()> {
    let mut index = 0;
    while index < line.len() {
        let mut value = line[index];
        if value == b'=' {
            index += 1;
            value = *line.get(index).context("truncated yEnc escape")?;
            value = value.wrapping_sub(64);
        }
        output.push(value.wrapping_sub(42));
        index += 1;
    }
    Ok(())
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn parse_fields(line: &[u8]) -> Result<Vec<(&str, &str)>> {
    let text = std::str::from_utf8(line).context("yEnc control line is not ASCII")?;
    Ok(text
        .split_ascii_whitespace()
        .skip(1)
        .filter_map(|field| field.split_once('='))
        .collect())
}

fn numeric(fields: &[(&str, &str)], key: &str) -> Result<u64> {
    text(fields, key)
        .with_context(|| format!("missing yEnc {key}"))?
        .parse()
        .with_context(|| format!("invalid yEnc {key}"))
}

fn text<'a>(fields: &'a [(&str, &str)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|(candidate, value)| (*candidate == key).then_some(*value))
}

#[cfg(test)]
mod tests {
    use crc32fast::hash;

    use super::decode_yenc;

    #[test]
    fn decodes_escaped_bytes_and_validates_crc() {
        let payload = [0_u8, 10, 13, 61, 255];
        let mut encoded = Vec::new();
        for byte in payload {
            let shifted = byte.wrapping_add(42);
            if matches!(shifted, 0 | 10 | 13 | 61) {
                encoded.push(b'=');
                encoded.push(shifted.wrapping_add(64));
            } else {
                encoded.push(shifted);
            }
        }
        let mut article = b"=ybegin line=128 size=5 name=sample.bin\r\n".to_vec();
        article.extend(encoded);
        article.extend(format!("\r\n=yend size=5 crc32={:08x}\r\n", hash(&payload)).as_bytes());
        let decoded = decode_yenc(&article).expect("decode article");
        assert_eq!(decoded.data, payload);
    }

    #[test]
    fn rejects_a_bad_crc() {
        let article = b"=ybegin line=128 size=1 name=x\r\n+\r\n=yend size=1 crc32=00000000\r\n";
        assert!(decode_yenc(article).is_err());
    }
}
