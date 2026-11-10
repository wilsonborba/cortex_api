use cortex_api::{
    core::{config::Config, process_numbers},
    domain::{
        models::{events::NativeEvent, extraction::DocumentFile},
        services::{events::channel, extraction::DocumentExtractor},
    },
};
use docx_rs::{Docx, Paragraph, Run, Table, TableCell, TableRow};
use pdfium_render::prelude::*;
use std::{io::Cursor, sync::Arc};
fn file(name: &str, content_type: &str, bytes: Vec<u8>) -> DocumentFile {
    DocumentFile {
        name: name.into(),
        content_type: content_type.into(),
        bytes: Arc::new(bytes),
    }
}
#[tokio::test]
async fn utf8_docx_table_failures_limits_and_other_files_continue() {
    let config = Config::default();
    assert_eq!(config.file_bytes, 25 * 1024 * 1024);
    assert_eq!(config.ocr_page_limit, 20);
    let extractor = DocumentExtractor::new(&config);
    let (sink, _rx, events) = channel(&config, None);
    let paragraph = |text: &str| Paragraph::new().add_run(Run::new().add_text(text));
    let doc = Docx::new()
        .add_paragraph(paragraph("A paragraph in English, Português, ภาษาไทย."))
        .add_table(Table::new(vec![TableRow::new(vec![
            TableCell::new().add_paragraph(paragraph("Cell one")),
            TableCell::new().add_paragraph(paragraph("Cell two")),
        ])]));
    let mut buffer = Cursor::new(vec![]);
    doc.build().pack(&mut buffer).unwrap();
    let results = extractor
        .extract_all(
            vec![
                file("bad.bin", "application/octet-stream", vec![0, 1]),
                file("large.txt", "text/plain", vec![b'a'; config.file_bytes + 1]),
                file("bad.txt", "text/plain", vec![255]),
                file(
                    "bad.docx",
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                    vec![0, 1],
                ),
                file(
                    "good.txt",
                    "text/plain; charset=utf-8",
                    "\u{feff}English Português ภาษาไทย".as_bytes().to_vec(),
                ),
                file(
                    "table.docx",
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                    buffer.into_inner(),
                ),
            ],
            &sink,
        )
        .await;
    assert_eq!(
        results[0].error.unwrap().number,
        process_numbers::FILE_TYPE.number
    );
    assert_eq!(
        results[1].error.unwrap().number,
        process_numbers::FILE_SIZE.number
    );
    assert_eq!(
        results[2].error.unwrap().number,
        process_numbers::FILE_EXTRACTION.number
    );
    assert_eq!(
        results[3].error.unwrap().number,
        process_numbers::FILE_EXTRACTION.number
    );
    assert!(results[4].error.is_none());
    assert_eq!(results[4].text(), "English Português ภาษาไทย");
    assert!(results[5].error.is_none());
    assert!(results[5].text().contains("A paragraph"));
    assert!(results[5].text().contains("Cell one"));
    assert!(results[5].text().contains("Cell two"));
    let exact = extractor
        .extract(
            file("exact.txt", "text/plain", vec![b'a'; config.file_bytes]),
            &sink,
        )
        .await;
    assert!(exact.error.is_none());
    eprintln!(
        "timing text_ms={} docx_ms={}",
        results[4].time_ms, results[5].time_ms
    );
    drop(sink);
    events.await.unwrap();
}
fn pdfium(config: &Config) -> Pdfium {
    Pdfium::new(
        Pdfium::bind_to_library(&config.pdfium_path).expect("run scripts/setup-native-files.sh"),
    )
}
fn text_pdf(pdfium: &Pdfium) -> Vec<u8> {
    let mut doc = pdfium.create_new_pdf().unwrap();
    let font = doc.fonts_mut().helvetica();
    {
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .unwrap();
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(40.0),
                PdfPoints::new(700.0),
                "Cortex memory document sample",
                font,
                PdfPoints::new(24.0),
            )
            .unwrap();
    }
    doc.save_to_bytes().unwrap()
}
fn scanned_pdf(pdfium: &Pdfium, count: usize) -> Vec<u8> {
    let text = text_pdf(pdfium);
    let source = pdfium.load_pdf_from_byte_slice(&text, None).unwrap();
    let image = source
        .pages()
        .get(0)
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().set_target_width(1240))
        .unwrap()
        .as_image()
        .unwrap();
    let mut doc = pdfium.create_new_pdf().unwrap();
    for _ in 0..count {
        let object =
            PdfPageImageObject::new_with_width(&doc, &image, PdfPoints::new(595.0)).unwrap();
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .unwrap();
        page.objects_mut().add_image_object(object).unwrap();
    }
    doc.save_to_bytes().unwrap()
}
#[tokio::test]
#[ignore = "requires the pinned local PDFium build from scripts/setup-native-files.sh and Tesseract eng+por+tha"]
async fn real_pdf_text_scan_and_ocr_page_limits() {
    let config = Config::default();
    let pdfium = pdfium(&config);
    let text = text_pdf(&pdfium);
    let scanned = scanned_pdf(&pdfium, 1);
    let over = scanned_pdf(&pdfium, 21);
    let mut blank = pdfium.create_new_pdf().unwrap();
    for _ in 0..20 {
        blank
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .unwrap();
    }
    let exact = blank.save_to_bytes().unwrap();
    let extractor = DocumentExtractor::new(&config);
    let (sink, mut rx, events) = channel(&config, None);
    let plain = extractor
        .extract(file("text.pdf", "application/pdf", text), &sink)
        .await;
    assert!(plain.error.is_none());
    assert!(plain.text().contains("Cortex memory document sample"));
    assert!(!plain.pages[0].ocr);
    let scan = extractor
        .extract(file("scan.pdf", "application/pdf", scanned), &sink)
        .await;
    assert!(scan.error.is_none());
    assert!(scan.pages[0].ocr);
    assert!(scan.text().contains("Cortex memory document sample"));
    let over = extractor
        .extract(file("over.pdf", "application/pdf", over), &sink)
        .await;
    assert_eq!(
        over.error.unwrap().number,
        process_numbers::FILE_OCR_LIMIT.number
    );
    assert!(over.pages.is_empty());
    let small_config = Config {
        ocr_render_width: 600,
        ocr_render_height: 850,
        ..config.clone()
    };
    let exact = DocumentExtractor::new(&small_config)
        .extract(file("exact.pdf", "application/pdf", exact), &sink)
        .await;
    assert!(exact.error.is_none());
    assert_eq!(exact.pages.len(), 20);
    assert!(exact.pages.iter().all(|p| p.ocr));
    let bad = extractor
        .extract(file("bad.pdf", "application/pdf", vec![1, 2]), &sink)
        .await;
    assert_eq!(
        bad.error.unwrap().number,
        process_numbers::FILE_EXTRACTION.number
    );
    eprintln!(
        "timing pdf_text_ms={} pdf_scan_ms={} reject_21_ocr_ms={}",
        plain.time_ms, scan.time_ms, over.time_ms
    );
    drop(sink);
    events.await.unwrap();
    let mut scan_steps = 0;
    while let Ok(event) = rx.try_recv() {
        if let NativeEvent::Step(step) = event.event
            && step.step == cortex_api::domain::models::events::StepName::OcrPage
        {
            if step.params["file_name"] == "scan.pdf" {
                assert_eq!(step.progress, Some([1, 1]));
                scan_steps += 1;
            } else {
                assert_eq!(step.params["file_name"], "exact.pdf");
            }
        }
    }
    assert_eq!(scan_steps, 1);
}
