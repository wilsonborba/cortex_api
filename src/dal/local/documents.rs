use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    domain::{
        models::{
            events::{NativeEvent, StepName, StepProgress},
            extraction::TextPage,
            pipeline::StageName,
        },
        services::events::EventSink,
    },
};
use docx_rs::{DocumentChild, TableCellContent, TableChild, TableRowChild};
use pdfium_render::prelude::*;

fn table_text(table: &docx_rs::Table, output: &mut String) {
    for TableChild::TableRow(row) in &table.rows {
        for (index, TableRowChild::TableCell(cell)) in row.cells.iter().enumerate() {
            if index > 0 {
                output.push('\t');
            }
            for content in &cell.children {
                match content {
                    TableCellContent::Paragraph(p) => {
                        output.push_str(&p.raw_text());
                        output.push('\n');
                    }
                    TableCellContent::Table(t) => table_text(t, output),
                    _ => {}
                }
            }
        }
        output.push('\n');
    }
}
pub fn docx(bytes: &[u8]) -> Result<Vec<TextPage>, ProcessNumber> {
    let doc = docx_rs::read_docx(bytes).map_err(|_| process_numbers::FILE_EXTRACTION)?;
    let mut text = String::new();
    for child in doc.document.children {
        match child {
            DocumentChild::Paragraph(p) => {
                text.push_str(&p.raw_text());
                text.push('\n');
            }
            DocumentChild::Table(t) => table_text(&t, &mut text),
            _ => {}
        }
    }
    Ok(vec![TextPage {
        number: 1,
        text,
        ocr: false,
    }])
}
pub fn progress(events: &EventSink, step: StepName, name: &str, page: u32, total: u32) {
    let mut event = StepProgress::new(StageName::Gather, step);
    event.params.insert("file_name".into(), name.into());
    event.params.insert("page".into(), page.into());
    event.params.insert("total".into(), total.into());
    event.progress = Some([page.into(), total.into()]);
    events.send(NativeEvent::Step(event));
}
pub fn pdf(
    config: &Config,
    name: &str,
    bytes: &[u8],
    events: &EventSink,
) -> Result<Vec<TextPage>, ProcessNumber> {
    let pdfium = match Pdfium::bind_to_library(&config.pdfium_path) {
        Ok(bindings) => Pdfium::new(bindings),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Pdfium::default(),
        Err(_) => return Err(process_numbers::FILE_EXTRACTION),
    };
    let doc = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|_| process_numbers::FILE_EXTRACTION)?;
    let total = doc.pages().len() as u32;
    let mut pages = vec![];
    let mut blank = vec![];
    for (index, page) in doc.pages().iter().enumerate() {
        progress(
            events,
            StepName::FileReadPage,
            name,
            index as u32 + 1,
            total,
        );
        let text = page
            .text()
            .map_err(|_| process_numbers::FILE_EXTRACTION)?
            .all();
        if text.trim().is_empty() {
            blank.push(index);
        }
        pages.push(TextPage {
            number: index as u32 + 1,
            text,
            ocr: false,
        });
    }
    // Reject an over-limit file before any OCR page is processed.
    if blank.len() > config.ocr_page_limit {
        return Err(process_numbers::FILE_OCR_LIMIT);
    }
    if blank.is_empty() {
        return Ok(pages);
    }
    let mut ocr = tesseract::Tesseract::new(None, Some(&config.ocr_languages))
        .map_err(|_| process_numbers::FILE_OCR)?;
    let render = PdfRenderConfig::new()
        .set_target_width(config.ocr_render_width)
        .set_maximum_height(config.ocr_render_height);
    for (ordinal, &index) in blank.iter().enumerate() {
        progress(
            events,
            StepName::OcrPage,
            name,
            ordinal as u32 + 1,
            blank.len() as u32,
        );
        let page = doc
            .pages()
            .get(index as PdfPageIndex)
            .map_err(|_| process_numbers::FILE_OCR)?;
        let image = page
            .render_with_config(&render)
            .and_then(|b| b.as_image())
            .map_err(|_| process_numbers::FILE_OCR)?
            .into_luma8();
        let width = i32::try_from(image.width()).map_err(|_| process_numbers::FILE_OCR)?;
        let height = i32::try_from(image.height()).map_err(|_| process_numbers::FILE_OCR)?;
        ocr = ocr
            .set_frame(image.as_raw(), width, height, 1, width)
            .map_err(|_| process_numbers::FILE_OCR)?
            .recognize()
            .map_err(|_| process_numbers::FILE_OCR)?;
        pages[index].text = ocr.get_text().map_err(|_| process_numbers::FILE_OCR)?;
        pages[index].ocr = true;
    }
    Ok(pages)
}
