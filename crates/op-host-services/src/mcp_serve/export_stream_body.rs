//! Stream the inner files JSON through its enclosing MCP text-string escape.
//! Base64's alphabet is safe at both JSON levels and goes straight to the
//! fixed HTTP chunk buffer; no batch Vec<Value> or base64 String is allocated.
use super::{Format, Plan};
use crate::export::{ExportError, RasterFormat};
use std::io::{self, Write};

struct Text<'a, W> {
    output: &'a mut W,
    first: bool,
}
impl<W: Write> Text<'_, W> {
    fn json(&mut self, value: &str) -> io::Result<()> {
        serde_json::to_writer(&mut *self, value).map_err(io::Error::other)
    }
    fn entry(
        &mut self,
        id: &str,
        format: &str,
        result: Result<Vec<u8>, ExportError>,
    ) -> io::Result<()> {
        if !self.first {
            self.write_all(b",")?;
        }
        self.first = false;
        match result {
            Ok(bytes) => {
                self.write_all(b"{\"bytes_base64\":\"")?;
                {
                    let mut encoder = base64::write::EncoderWriter::new(
                        &mut self.output,
                        &base64::engine::general_purpose::STANDARD,
                    );
                    encoder.write_all(&bytes)?;
                    encoder.finish()?;
                }
                self.write_all(b"\",\"format\":")?;
                self.json(format)?;
                self.write_all(b",\"nodeId\":")?;
                self.json(id)?;
                self.write_all(b"}")?;
            }
            Err(error) => {
                self.write_all(b"{\"error\":")?;
                self.json(&error.to_string())?;
                self.write_all(b",\"format\":")?;
                self.json(format)?;
                self.write_all(b",\"nodeId\":")?;
                self.json(id)?;
                self.write_all(b"}")?;
            }
        }
        // The raster worker gets its ACK only after this entry reaches the
        // socket writer. A failed flush returns false and disconnects ACK.
        self.flush()
    }
}
impl<W: Write> Write for Text<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Literal Unicode stays UTF8, just as the standard MCP JSON escaper.
        // JSON serializers already escape control bytes in metadata; retaining
        // this complete mapping also makes direct inner delimiters safe.
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            if matches!(byte, b'"' | b'\\' | 0..=31) {
                self.output.write_all(&bytes[start..index])?;
                match byte {
                    b'"' => self.output.write_all(b"\\\"")?,
                    b'\\' => self.output.write_all(b"\\\\")?,
                    _ => {
                        const HEX: &[u8; 16] = b"0123456789abcdef";
                        self.output.write_all(&[
                            b'\\',
                            b'u',
                            b'0',
                            b'0',
                            HEX[(byte >> 4) as usize],
                            HEX[(byte & 15) as usize],
                        ])?;
                    }
                }
                start = index + 1;
            }
        }
        self.output.write_all(&bytes[start..])?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub(super) fn render<W: Write>(plan: Plan, output: &mut W) -> io::Result<()> {
    let Plan {
        scene,
        ids,
        format,
        scale,
    } = plan;
    let mut text = Text {
        output,
        first: true,
    };
    text.write_all(b"{\"files\":[")?;
    match format {
        Format::Raster(format) => {
            let label = match format {
                RasterFormat::Png => "png",
                RasterFormat::Jpeg => "jpeg",
                RasterFormat::Webp => "webp",
            };
            let mut failed = None;
            scene.render_active_nodes_until(&ids, format, scale, |id, result| {
                match text.entry(id, label, result) {
                    Ok(()) => true,
                    Err(error) => {
                        failed = Some(error);
                        false
                    }
                }
            });
            if let Some(error) = failed {
                return Err(error);
            }
        }
        Format::Svg => {
            // prepare() rejects no-active-page SVG before starting the response,
            // matching inline's whole-tool error rather than inventing entries.
            let page = scene
                .active_page()
                .ok_or_else(|| io::Error::other("export stream lost its immutable active page"))?;
            let batch = op_render_export::NodeSvgBatch::new(page, &ids);
            for id in &ids {
                text.entry(id, "svg", batch.render(id, scale))?;
            }
        }
        Format::Pdf => {
            if let Some(page) = scene.active_page() {
                let batch = op_render_export::SceneSelectionBatch::new(
                    page,
                    ids.iter().map(String::as_str),
                );
                for id in &ids {
                    let result = batch
                        .get(id.as_str())
                        .ok_or_else(|| ExportError::NodeNotFoundOnPage {
                            node_id: id.clone(),
                            page_id: page.id.clone(),
                        })
                        .and_then(|selected| {
                            crate::export_pdf::render_selected_node_pdf_bytes_scaled(
                                &selected, scale,
                            )
                        });
                    text.entry(id, "pdf", result)?;
                }
            } else {
                for id in &ids {
                    text.entry(id, "pdf", Err(ExportError::NoActivePage))?;
                }
            }
        }
    }
    text.write_all(b"]}")?;
    text.flush()
}
