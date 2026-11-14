//! Offline `.fig` → `.op` conversion for the managed daemon: the VS Code
//! extension cannot parse fig-kiwi, so it POSTs the raw bytes here and
//! boots the returned document JSON through the normal open-document push.

use base64::Engine;
use std::borrow::Cow;

use crate::figma_convert_error::FigmaConvertError;

/// Every fallible step of this module fails with [`FigmaConvertError`].
type Result<T> = std::result::Result<T, FigmaConvertError>;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertRequest<'a> {
    name: String,
    #[serde(borrow)]
    bytes_b64: Cow<'a, str>,
}

fn decode_b64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| FigmaConvertError::BadBase64 {
            detail: e.to_string(),
        })
}

/// Same STANDARD alphabet `op_figma::image_resolver::blob_to_data_url` uses
/// for its data-url encoding. Only the tests below need to encode base64
/// (production traffic sends it pre-encoded from the VS Code extension), so
/// this stays `#[cfg(test)]`-gated rather than dead-code-linted in release
/// builds.
#[cfg(test)]
fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn convert_fig_json(body: &str) -> Result<String> {
    let (name, bytes) = decode_request(body)?;
    convert_decoded(name, bytes)
}

/// Release the HTTP JSON/base64 allocation before building the Figma tree.
pub(crate) fn convert_fig_json_owned(body: String) -> Result<String> {
    let (name, bytes) = decode_request(&body)?;
    drop(body);
    convert_decoded(name, bytes)
}

fn decode_request(body: &str) -> Result<(String, Vec<u8>)> {
    let req: ConvertRequest =
        serde_json::from_str(body).map_err(|e| FigmaConvertError::BadRequest {
            detail: e.to_string(),
        })?;
    let bytes = decode_b64(&req.bytes_b64)?;
    Ok((req.name, bytes))
}

fn convert_decoded(name: String, bytes: Vec<u8>) -> Result<String> {
    let import =
        op_figma::prepare_fig_binary_owned(bytes, &name, op_figma::FigLayoutMode::Preserve)
            .and_then(|prepared| prepared.into_all_pages_with_images(None))
            .map_err(|e| FigmaConvertError::Parse {
                name,
                detail: format!("{e:?}"),
            })?;
    let thumbnails = jian_ops_schema::image_thumbs::capture_snapshot();
    let mut response = Vec::new();
    response.extend_from_slice(br#"{"ok":true,"doc":"#);
    jian_ops_schema::image_table::write_document_with_extension(
        &mut response,
        &import.document,
        &thumbnails,
        "editorMeta",
        &op_pen_loader::EditorMeta {
            active_page_index: 0,
            preserve_authored_geometry: true,
            // A Figma import is whatever the source file was; nothing here
            // establishes it as a deck or a card set, nor pins a style.
            scenario: None,
            work_family: None,
            pinned_style_guide: None,
            share_recipe: None,
            imported_from: None,
        },
    )
    .map_err(|error| FigmaConvertError::Encode {
        detail: error.to_string(),
    })?;
    response.extend_from_slice(br#", "warnings":"#);
    serde_json::to_writer(&mut response, &import.warnings).map_err(|error| {
        FigmaConvertError::Encode {
            detail: error.to_string(),
        }
    })?;
    response.push(b'}');
    String::from_utf8(response).map_err(|error| FigmaConvertError::Encode {
        detail: error.to_string(),
    })
}

// Happy-path coverage: op-figma's only fig-kiwi fixture builder
// (`binary_e2e_tests::build_fig`) is a private `#[cfg(test)]` helper with
// no `pub` export, so it isn't reachable from here and isn't worth
// duplicating (~200 lines of hand-rolled Kiwi wire encoding) just for this
// route. The happy path (a real `.fig` converting end-to-end through this
// endpoint) is covered by the Task 10 manual matrix instead.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_json_and_bad_base64_and_non_fig_bytes() {
        assert!(convert_fig_json("not json").is_err());
        assert!(convert_fig_json(r#"{"name":"a.fig","bytesB64":"!!!"}"#).is_err());
        let not_fig = base64_encode(b"plain text, not fig-kiwi");
        let body = format!(r#"{{"name":"a.fig","bytesB64":"{not_fig}"}}"#);
        assert!(convert_fig_json(&body).is_err());
    }

    #[test]
    fn base64_borrows_normal_json_and_accepts_escaped_payloads() {
        let raw = r#"{"name":"a.fig","bytesB64":"/w=="}"#;
        let request: ConvertRequest = serde_json::from_str(raw).unwrap();
        assert!(matches!(request.bytes_b64, Cow::Borrowed(_)));
        assert_eq!(decode_request(raw).unwrap(), ("a.fig".into(), vec![255]));
        let escaped = r#"{"name":"a.fig","bytesB64":"\/w=="}"#;
        assert_eq!(
            decode_request(escaped).unwrap(),
            ("a.fig".into(), vec![255])
        );
        for body in ["not json", raw, escaped] {
            assert_eq!(
                convert_fig_json(body).unwrap_err().to_string(),
                convert_fig_json_owned(body.into()).unwrap_err().to_string()
            );
        }
    }
}
