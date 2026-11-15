//! Same native PDF page sequence for Vec and atomic file sinks.
use crate::ExportError;
use std::io::Write;

pub(super) struct Pages<'a> {
    document: Option<skia_safe::Document<'a>>,
}
impl Pages<'_> {
    pub(super) fn page(
        &mut self,
        width: f32,
        height: f32,
        paint: impl FnOnce(&skia_safe::Canvas),
    ) -> Result<(), ExportError> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(ExportError::NonFiniteOutputSize);
        }
        let document = self.document.take().ok_or(ExportError::PdfEncoderEmpty)?;
        let mut page = document.begin_page(skia_safe::Size::new(width, height), None);
        paint(page.canvas());
        self.document = Some(page.end_page());
        Ok(())
    }
}
pub(super) fn write(
    output: &mut dyn Write,
    emit: impl FnOnce(&mut Pages<'_>) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    let mut checked = crate::encoding::CheckedWriter::new(output);
    {
        let metadata = super::metadata();
        let document = skia_safe::pdf::new_document(&mut checked, Some(&metadata));
        let mut pages = Pages {
            document: Some(document),
        };
        crate::with_batch_preparation(|| emit(&mut pages))?;
        let document = pages.document.take().ok_or(ExportError::PdfEncoderEmpty)?;
        if document.pages() == 0 {
            document.abort();
            return Err(ExportError::PdfEncoderEmpty);
        }
        document.close();
    }
    checked.finish_pdf()
}
pub(super) fn bytes(
    emit: impl FnOnce(&mut Pages<'_>) -> Result<(), ExportError>,
) -> Result<Vec<u8>, ExportError> {
    let mut output = Vec::new();
    write(&mut output, emit)?;
    Ok(output)
}
