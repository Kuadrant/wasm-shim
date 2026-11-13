use std::borrow::Cow;
use std::io::{self, Read};
use std::num::NonZeroU32;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use flate2::bufread::GzDecoder;
use serde::{Deserialize, Deserializer};

pub const DEFAULT_MAX_DECOMPRESSED_BYTES: usize = 32 * 1024 * 1024;

/// Returns plain JSON or decompresses a `gzip+base64` envelope
///
/// # Errors
/// Fails on invalid input, oversized output, or allocation failure
pub fn decode_configuration(input: &[u8]) -> io::Result<Cow<'_, [u8]>> {
    let Some(Envelope {
        data,
        max_decompressed_bytes,
    }) = parse_envelope(input)?
    else {
        return Ok(Cow::Borrowed(input));
    };
    let compressed = STANDARD.decode(data.as_bytes()).map_err(invalid_data)?;
    // Free the payload early
    drop(data);
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if count > max_decompressed_bytes - output.len() {
            return Err(invalid_data(format!(
                "decompressed configuration exceeds {max_decompressed_bytes} bytes; set a larger maxDecompressedBytes to raise the limit"
            )));
        }
        output
            .try_reserve(count)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        output.extend_from_slice(&buffer[..count]);
    }
    if !decoder.into_inner().is_empty() {
        return Err(invalid_data(
            "configuration gzip stream has trailing data or additional members",
        ));
    }
    // Free compressed bytes early
    drop(compressed);
    if parse_envelope(&output)?.is_some() {
        return Err(invalid_data(
            "nested configuration envelopes are not supported",
        ));
    }
    Ok(Cow::Owned(output))
}

#[derive(Deserialize)]
struct MaybeEnvelope<'a> {
    #[serde(default)]
    encoding: Field<Encoding>,
    #[serde(default, borrow)]
    data: Field<Payload<'a>>,
    #[serde(default, rename = "maxDecompressedBytes")]
    max_decompressed_bytes: Field<MaxDecompressedBytes>,
}

struct Envelope<'a> {
    data: Cow<'a, str>,
    max_decompressed_bytes: usize,
}

#[derive(Deserialize)]
#[serde(try_from = "u32")]
struct MaxDecompressedBytes(NonZeroU32);

impl TryFrom<u32> for MaxDecompressedBytes {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        NonZeroU32::new(value)
            .filter(|value| value.get() <= i32::MAX as u32)
            .map(Self)
            .ok_or("maxDecompressedBytes must be between 1 and 2147483647")
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
struct Payload<'a>(#[serde(borrow)] Cow<'a, str>);

#[derive(Deserialize)]
enum Encoding {
    #[serde(rename = "gzip+base64")]
    GzipBase64,
}

#[derive(Default)]
enum Field<T> {
    #[default]
    Missing,
    Present(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Present)
    }
}

fn parse_envelope(input: &[u8]) -> io::Result<Option<Envelope<'_>>> {
    // Serde structs also accept sequences
    if input.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(invalid_data("configuration must be a JSON object"));
    }
    let fields: MaybeEnvelope<'_> = serde_json::from_slice(input).map_err(invalid_data)?;
    match fields {
        MaybeEnvelope {
            encoding: Field::Missing,
            data: Field::Missing,
            max_decompressed_bytes: Field::Missing,
        } => Ok(None),
        MaybeEnvelope {
            encoding: Field::Present(Encoding::GzipBase64),
            data: Field::Present(Payload(data)),
            max_decompressed_bytes,
        } => {
            let max_decompressed_bytes = match max_decompressed_bytes {
                Field::Missing => DEFAULT_MAX_DECOMPRESSED_BYTES,
                Field::Present(limit) => usize::try_from(limit.0.get()).map_err(invalid_data)?,
            };
            Ok(Some(Envelope {
                data,
                max_decompressed_bytes,
            }))
        }
        _ => Err(invalid_data("envelope requires both encoding and data")),
    }
}

fn invalid_data(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::error::Error;
    use std::io::Write;

    type TestResult = Result<(), Box<dyn Error>>;

    fn gzip(input: &[u8]) -> io::Result<Vec<u8>> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(input)?;
        encoder.finish()
    }

    fn envelope(compressed: &[u8]) -> Vec<u8> {
        serde_json::json!({"encoding": "gzip+base64", "data": STANDARD.encode(compressed)})
            .to_string()
            .into_bytes()
    }

    fn envelope_with_limit(compressed: &[u8], limit: usize) -> Vec<u8> {
        serde_json::json!({
            "encoding": "gzip+base64",
            "data": STANDARD.encode(compressed),
            "maxDecompressedBytes": limit
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn defaults_to_32_mib_when_limit_is_omitted() -> TestResult {
        let input = envelope(&gzip(b"{}")?);
        let parsed = parse_envelope(&input)?.ok_or("expected envelope")?;
        assert_eq!(parsed.max_decompressed_bytes, 33_554_432);
        Ok(())
    }

    #[test]
    fn enforces_smaller_limits_at_the_exact_boundary() -> TestResult {
        for (input, accepted) in [(b"{}".as_slice(), true), (b"{} ", false)] {
            let encoded = envelope_with_limit(&gzip(input)?, 2);
            let result = decode_configuration(&encoded);
            if accepted {
                assert_eq!(result?.as_ref(), input);
            } else {
                assert!(
                    matches!(result, Err(error) if error.to_string() == "decompressed configuration exceeds 2 bytes; set a larger maxDecompressedBytes to raise the limit")
                );
            }
        }
        Ok(())
    }

    #[test]
    fn accepts_output_above_default_with_an_override() -> TestResult {
        let mut input = vec![b' '; DEFAULT_MAX_DECOMPRESSED_BYTES + 1];
        input[..2].copy_from_slice(b"{}");
        let compressed = gzip(&input)?;
        let encoded = envelope_with_limit(&compressed, input.len());
        assert!(decode_configuration(&encoded)?.as_ref() == input);
        Ok(())
    }

    #[test]
    fn rejects_invalid_limits() -> TestResult {
        let compressed = STANDARD.encode(gzip(b"{}")?);
        for limit in [
            "null",
            "0",
            "-1",
            "1.5",
            "2.0",
            "1e3",
            "\"64Mi\"",
            "2147483648",
            "4294967296",
        ] {
            let input = format!(
                r#"{{"encoding":"gzip+base64","data":"{compressed}","maxDecompressedBytes":{limit}}}"#
            );
            assert!(
                matches!(decode_configuration(input.as_bytes()), Err(error) if error.kind() == io::ErrorKind::InvalidData),
                "accepted {limit}"
            );
        }
        let duplicate = format!(
            r#"{{"encoding":"gzip+base64","data":"{compressed}","maxDecompressedBytes":2,"maxDecompressedBytes":3}}"#
        );
        assert!(decode_configuration(duplicate.as_bytes()).is_err());
        assert!(decode_configuration(br#"{"maxDecompressedBytes":2}"#).is_err());
        Ok(())
    }

    #[test]
    fn accepts_the_largest_supported_limit_without_preallocating_it() -> TestResult {
        let encoded = envelope_with_limit(&gzip(b"{}")?, 2_147_483_647);
        assert_eq!(decode_configuration(&encoded)?.as_ref(), b"{}");
        Ok(())
    }

    #[test]
    fn validates_the_gzip_trailer_at_an_overridden_limit() -> TestResult {
        let mut compressed = gzip(b"{}")?;
        let offset = compressed.len() - 8;
        compressed[offset] ^= 1;
        let encoded = envelope_with_limit(&compressed, 2);
        assert!(decode_configuration(&encoded).is_err());
        Ok(())
    }

    #[test]
    fn borrows_unescaped_payload_from_the_input() -> TestResult {
        let input = br#"{"encoding":"gzip+base64","data":"e30="}"#;
        let Envelope { data, .. } = parse_envelope(input)?.ok_or("expected envelope")?;
        assert!(matches!(data, Cow::Borrowed(_)));
        assert_eq!(data, "e30=");
        let offset = input
            .windows(4)
            .position(|part| part == b"e30=")
            .ok_or("missing payload in fixture")?;
        assert_eq!(data.as_ptr(), input[offset..].as_ptr());
        Ok(())
    }

    #[test]
    fn owns_unescaped_result_of_an_escaped_payload() -> TestResult {
        let input = br#"{"encoding":"gzip+base64","data":"\u006530="}"#;
        let Envelope { data, .. } = parse_envelope(input)?.ok_or("expected envelope")?;
        assert!(matches!(data, Cow::Owned(_)));
        assert_eq!(data, "e30=");
        Ok(())
    }

    #[test]
    fn plain_configuration_is_borrowed_unchanged() -> TestResult {
        let input = b" {\n\"services\": {}, \"actionSets\": [], \"unknown\": true}\n";
        let output = decode_configuration(input)?;
        assert!(matches!(output, Cow::Borrowed(_)));
        assert_eq!(output.as_ptr(), input.as_ptr());
        assert_eq!(output.as_ref(), input);
        Ok(())
    }

    #[test]
    fn compressed_configuration_round_trips_exactly() -> TestResult {
        let input = b" {\n\"services\": {}, \"actionSets\": []}\n";
        let encoded = envelope(&gzip(input)?);
        let output = decode_configuration(&encoded)?;
        assert!(matches!(output, Cow::Owned(_)));
        assert_eq!(output.as_ref(), input);
        let _: crate::configuration::PluginConfiguration = serde_json::from_slice(&output)?;
        Ok(())
    }

    #[test]
    fn decodes_an_independent_python_gzip_fixture() -> TestResult {
        let input = br#"{"encoding":"gzip+base64","data":"H4sIAAAAAAAA/1Oo5lIqTi0qy0xOLVayUqiu1VFQSkwuyczPC04tAYlEx9ZyAQC/QnO7JQAAAA=="}"#;
        assert_eq!(
            decode_configuration(input)?.as_ref(),
            b" {\n\"services\": {}, \"actionSets\": []}\n"
        );
        Ok(())
    }

    #[test]
    fn accepts_json_escaped_base64_and_either_field_order() -> TestResult {
        let input = br#"{"data":"\u00484sIAAAAAAAA\/1Oo5lIqTi0qy0xOLVayUqiu1VFQSkwuyczPC04tAYlEx9ZyAQC\/QnO7JQAAAA==","encoding":"gzip+base64"}"#;
        assert_eq!(
            decode_configuration(input)?.as_ref(),
            b" {\n\"services\": {}, \"actionSets\": []}\n"
        );
        Ok(())
    }

    #[test]
    fn leaves_semantic_configuration_validation_to_the_caller() -> TestResult {
        let input = envelope(&gzip(b"{}")?);
        let decoded = decode_configuration(&input)?;
        assert_eq!(decoded.as_ref(), b"{}");
        assert!(
            serde_json::from_slice::<crate::configuration::PluginConfiguration>(&decoded).is_err()
        );
        Ok(())
    }

    #[test]
    fn malformed_envelopes_never_fall_back_to_plain() {
        for input in [
            r#"{"encoding":"gzip+base64"}"#,
            r#"{"data":""}"#,
            r#"{"services":{},"actionSets":[],"data":""}"#,
            r#"{"encoding":"gzip+base64","encoding":"gzip+base64","data":""}"#,
            r#"{"encoding":"gzip+base64","data":"","data":""}"#,
            r#"{"encoding":"gzip+base64","data":"","d\u0061ta":""}"#,
            r#"{"encoding":"other","data":""}"#,
            r#"{"encoding":null,"data":""}"#,
            r#"{"encoding":null,"data":null,"services":{},"actionSets":[]}"#,
            r#"{"data":null,"services":{},"actionSets":[]}"#,
            r#"{"encoding":"gzip+base64","data":null}"#,
            r#"{"encoding":"gzip+base64","data":123}"#,
        ] {
            assert!(
                matches!(
                    decode_configuration(input.as_bytes()),
                    Err(error) if error.kind() == io::ErrorKind::InvalidData
                ),
                "accepted malformed envelope: {input}"
            );
        }
    }

    #[test]
    fn ignores_envelope_metadata_and_uses_only_the_decoded_configuration() -> TestResult {
        let payload = br#"{"services":{},"actionSets":[]}"#;
        let mut input: serde_json::Value = serde_json::from_slice(&envelope(&gzip(payload)?))?;
        input["metadata"] = serde_json::json!({"note":"optional", "length":payload.len()});
        input["services"] = serde_json::json!({"ignored":"outer configuration"});
        input["actionSets"] = serde_json::json!(["also ignored"]);
        let encoded = serde_json::to_vec(&input)?;
        assert_eq!(decode_configuration(&encoded)?.as_ref(), payload);
        Ok(())
    }

    #[test]
    fn rejects_invalid_base64_and_missing_padding() {
        for data in ["!", "AA", "AB==", "AA==\n"] {
            let input = serde_json::json!({"encoding":"gzip+base64", "data":data}).to_string();
            assert!(matches!(
                decode_configuration(input.as_bytes()),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ));
        }
    }

    #[test]
    fn rejects_invalid_corrupt_and_truncated_gzip() -> TestResult {
        let compressed = gzip(b"{}")?;
        let mut corrupt_crc = compressed.clone();
        let crc_offset = corrupt_crc.len() - 8;
        corrupt_crc[crc_offset] ^= 1;
        let mut corrupt_size = compressed.clone();
        let size_offset = corrupt_size.len() - 4;
        corrupt_size[size_offset] ^= 1;
        for invalid in [b"not gzip".to_vec(), Vec::new(), corrupt_crc, corrupt_size] {
            assert!(decode_configuration(&envelope(&invalid)).is_err());
        }
        for length in 0..compressed.len() {
            assert!(decode_configuration(&envelope(&compressed[..length])).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_additional_gzip_members_and_trailing_bytes() -> TestResult {
        let compressed = gzip(b"{}")?;
        for tail in [gzip(b"{}")?, gzip(b"")?, vec![0], b"trailing".to_vec()] {
            let mut invalid = compressed.clone();
            invalid.extend_from_slice(&tail);
            assert!(matches!(
                decode_configuration(&envelope(&invalid)),
                Err(error) if error.to_string().contains("trailing data")
            ));
        }
        Ok(())
    }

    #[test]
    fn rejects_nested_envelopes_even_if_incomplete() -> TestResult {
        for input in [
            br#"{"encoding":"gzip+base64","data":""}"#.as_slice(),
            br#"{"data":""}"#,
        ] {
            assert!(decode_configuration(&envelope(&gzip(input)?)).is_err());
        }
        Ok(())
    }

    #[test]
    fn requires_a_single_json_object_inside_and_outside() -> TestResult {
        for input in [
            b"{} {}".as_slice(),
            b"null",
            b"[]",
            b"[null,null]",
            br#"["gzip+base64", ""]"#,
            b"\xff",
            b"{",
            b"",
        ] {
            assert!(matches!(
                decode_configuration(input),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ));
            assert!(matches!(
                decode_configuration(&envelope(&gzip(input)?)),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ));
        }
        let mut input = envelope(&gzip(b"{}")?);
        input.extend_from_slice(b" {}");
        assert!(matches!(
            decode_configuration(&input),
            Err(error) if error.kind() == io::ErrorKind::InvalidData
        ));
        Ok(())
    }

    #[test]
    fn detects_escaped_keys_without_treating_nested_fields_as_envelopes() -> TestResult {
        assert!(matches!(
            decode_configuration(br#"{"enc\u006fding":"bad"}"#),
            Err(error) if error.kind() == io::ErrorKind::InvalidData
        ));
        let input = br#"{"services":{"encoding":{"data":"value"}},"actionSets":[]}"#;
        assert_eq!(decode_configuration(input)?.as_ref(), input);
        Ok(())
    }

    #[test]
    fn accepts_large_encoded_input_within_the_decoded_limit() -> TestResult {
        let mut input = vec![b' '; 3 * 1024 * 1024];
        input[..2].copy_from_slice(b"{}");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::none());
        encoder.write_all(&input)?;
        let encoded = envelope(&encoder.finish()?);
        assert!(encoded.len() > 4 * 1024 * 1024);
        assert!(decode_configuration(&encoded)?.as_ref() == input);
        Ok(())
    }

    #[test]
    fn decoded_limit_is_inclusive_and_plain_input_is_not_capped() -> TestResult {
        for length in [
            DEFAULT_MAX_DECOMPRESSED_BYTES,
            DEFAULT_MAX_DECOMPRESSED_BYTES + 1,
        ] {
            let mut input = vec![b' '; length];
            input[..2].copy_from_slice(b"{}");
            assert!(matches!(decode_configuration(&input)?, Cow::Borrowed(_)));
            let encoded = envelope(&gzip(&input)?);
            assert!(encoded.len() < 100_000);
            let result = decode_configuration(&encoded);
            if length == DEFAULT_MAX_DECOMPRESSED_BYTES {
                assert!(result?.as_ref() == input);
            } else {
                assert!(matches!(result, Err(error) if error.to_string().contains("exceeds")));
            }
        }
        Ok(())
    }

    #[test]
    fn corrupt_trailer_at_exact_output_limit_is_still_validated() -> TestResult {
        let mut input = vec![b' '; DEFAULT_MAX_DECOMPRESSED_BYTES];
        input[..2].copy_from_slice(b"{}");
        let mut compressed = gzip(&input)?;
        let offset = compressed.len() - 8;
        compressed[offset] ^= 1;
        assert!(decode_configuration(&envelope(&compressed)).is_err());
        Ok(())
    }

    #[test]
    fn errors_do_not_display_the_payload() {
        let input = br#"{"encoding":"gzip+base64","data":"private payload"}"#;
        assert!(
            matches!(decode_configuration(input), Err(error) if !error.to_string().contains("private payload"))
        );
    }
}
