//! Turns attachments into something a language model can read: text for documents,
//! spreadsheets, slides and mail, or the image itself.

use crate::model::AttachmentContent;
use crate::parse;
use anyhow::{anyhow, Result};
use base64::Engine as _;
use std::io::{Cursor, Read};

/// Images larger than this are not sent to a model.
const MAX_IMAGE: usize = 20 * 1024 * 1024;
/// Uncompressed size read from one file inside an Office document or archive.
const MAX_ENTRY: u64 = 50 * 1024 * 1024;
/// Total uncompressed bytes allowed across all entries of a ZIP-based document or spreadsheet.
const MAX_ZIP_TOTAL: u64 = 50 * 1024 * 1024;

/// The content of attachment `index` of a raw message. Text comes back from `offset` (in
/// characters), at most `max_chars` of it, so long documents can be read in parts.
pub fn attachment_content(raw: &[u8], index: usize, offset: usize, max_chars: usize) -> Result<AttachmentContent> {
    let (filename, mime, bytes) = parse::attachment_part(raw, index).ok_or_else(|| anyhow!("No attachment {index} in this message"))?;
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let size = bytes.len();
    let unsupported = |reason: &str| AttachmentContent::Unsupported {
        filename: filename.clone(),
        mime: mime.clone(),
        size,
        reason: reason.to_string(),
    };

    if let Some(img) = image_mime(&mime, &ext) {
        if size > MAX_IMAGE {
            return Ok(unsupported("The image is larger than 20 MB."));
        }
        return Ok(AttachmentContent::Image {
            filename,
            mime: img.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
            size,
        });
    }

    let extracted: Result<Option<(String, String, Option<usize>)>> = (|| {
        Ok(Some(match (mime.as_str(), ext.as_str()) {
            ("application/pdf", _) | (_, "pdf") => {
                let pages = pdf_pages(&bytes)?;
                let n = pages.len();
                let text: String = pages
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("--- Page {} ---\n{}\n", i + 1, tidy(p)))
                    .collect();
                if pages.iter().all(|p| p.trim().is_empty()) {
                    return Err(anyhow!("This PDF has no text layer (it is probably a scan or images)."));
                }
                ("pdf".into(), text, Some(n))
            }
            (m, e) if e == "docx" || m.contains("wordprocessingml") => ("docx".into(), docx_text(&bytes)?, None),
            (m, e) if e == "pptx" || m.contains("presentationml") => {
                let (text, n) = pptx_text(&bytes)?;
                ("pptx".into(), text, Some(n))
            }
            (m, e)
                if matches!(e, "xlsx" | "xlsm" | "xlsb" | "xls" | "ods")
                    || m.contains("spreadsheetml")
                    || m == "application/vnd.ms-excel"
                    || m == "application/vnd.oasis.opendocument.spreadsheet" =>
            {
                let (text, n) = spreadsheet_text(&bytes)?;
                ("spreadsheet".into(), text, Some(n))
            }
            ("message/rfc822", _) | (_, "eml") => ("email".into(), email_text(&bytes), None),
            (m, e) if m == "text/html" || matches!(e, "html" | "htm") => ("html".into(), html_text(&String::from_utf8_lossy(&bytes)), None),
            (m, e) if m == "application/zip" || e == "zip" => ("zip listing".into(), zip_listing(&bytes)?, None),
            (m, e) if is_text(m, e) => ("text".into(), String::from_utf8_lossy(&bytes).replace("\r\n", "\n"), None),
            _ => return Ok(None),
        }))
    })();

    match extracted {
        Ok(Some((format, text, parts))) => {
            let total_chars = text.chars().count();
            let offset = offset.min(total_chars);
            let text: String = text.chars().skip(offset).take(max_chars.max(1)).collect();
            Ok(AttachmentContent::Text { filename, mime, format, text, total_chars, offset, parts })
        }
        Ok(None) => Ok(unsupported(&format!("Mailvern can't read {} files.", if ext.is_empty() { mime.clone() } else { format!(".{ext}") }))),
        Err(e) => Ok(unsupported(&format!("{e:#}"))),
    }
}

fn image_mime(mime: &str, ext: &str) -> Option<&'static str> {
    match (mime, ext) {
        ("image/png", _) | (_, "png") => Some("image/png"),
        ("image/jpeg" | "image/jpg" | "image/pjpeg", _) | (_, "jpg" | "jpeg") => Some("image/jpeg"),
        ("image/gif", _) | (_, "gif") => Some("image/gif"),
        ("image/webp", _) | (_, "webp") => Some("image/webp"),
        _ => None,
    }
}

fn is_text(mime: &str, ext: &str) -> bool {
    mime.starts_with("text/")
        || matches!(mime, "application/json" | "application/xml" | "application/ics" | "application/x-yaml" | "application/yaml" | "application/javascript")
        || matches!(
            ext,
            "txt" | "csv" | "tsv" | "md" | "markdown" | "json" | "xml" | "ics" | "vcf" | "log" | "yaml" | "yml" | "toml" | "ini" | "sql" | "rtf"
        )
}

/// Collapses runs of blank lines and trailing spaces left by extraction.
pub(crate) fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn pdf_pages(bytes: &[u8]) -> Result<Vec<String>> {
    // The PDF parser can panic on unusual files; treat that like any other read failure.
    let bytes = bytes.to_vec();
    match std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem_by_pages(&bytes)) {
        Ok(Ok(pages)) => Ok(pages),
        Ok(Err(e)) => Err(anyhow!("Couldn't read this PDF: {e}")),
        Err(_) => Err(anyhow!("Couldn't read this PDF.")),
    }
}

fn zip_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str, budget: &mut u64) -> Result<Option<String>> {
    if *budget == 0 {
        return Ok(None);
    }
    let Ok(f) = zip.by_name(name) else { return Ok(None) };
    let cap = (*budget).min(MAX_ENTRY);
    let mut buf = Vec::new();
    f.take(cap).read_to_end(&mut buf)?;
    *budget = budget.saturating_sub(buf.len() as u64);
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Checks declared uncompressed size of a ZIP archive to reject zip bombs before handing
/// it to libraries (like `calamine`) that decompress entries without a size cap.
fn check_zip_total(bytes: &[u8]) -> Result<()> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Ok(());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow!("Couldn't open this archive: {e}"))?;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i).map_err(|e| anyhow!("Couldn't inspect archive entry: {e}"))?;
        total = total.saturating_add(f.size());
        if total > MAX_ZIP_TOTAL {
            return Err(anyhow!("This file is too large to read safely (exceeds 50 MB uncompressed)."));
        }
    }
    Ok(())
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text of an Office XML part. `text_tag` holds the words ("w:t", "a:t"); `breaks` maps
/// tags to what they add: (tag, on open or self-closing, on close).
fn office_xml_text(xml: &str, text_tag: &str, breaks: &[(&str, &str, &str)]) -> String {
    let mut out = String::new();
    let mut in_text = false;
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        if in_text {
            out.push_str(&unescape_xml(&rest[..lt]));
        }
        let Some(gt) = rest[lt..].find('>') else { break };
        let tag = &rest[lt + 1..lt + gt];
        rest = &rest[lt + gt + 1..];
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        if name == text_tag {
            in_text = !closing && !self_closing;
            continue;
        }
        if let Some((_, open, close)) = breaks.iter().find(|(t, _, _)| *t == name) {
            out.push_str(if closing { close } else { open });
        }
    }
    out
}

fn docx_text(bytes: &[u8]) -> Result<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow!("Couldn't open this Word file: {e}"))?;
    let mut budget = MAX_ZIP_TOTAL;
    let xml = zip_entry(&mut zip, "word/document.xml", &mut budget)?.ok_or_else(|| anyhow!("This Word file has no document body."))?;
    let breaks = [("w:p", "", "\n"), ("w:tab", "\t", ""), ("w:br", "\n", ""), ("w:cr", "\n", ""), ("w:tc", "", " | "), ("w:tr", "", "\n")];
    // A paragraph ending inside a table cell shouldn't split the row.
    Ok(tidy(&office_xml_text(&xml, "w:t", &breaks).replace("\n | ", " | ")))
}

fn pptx_text(bytes: &[u8]) -> Result<(String, usize)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow!("Couldn't open this PowerPoint file: {e}"))?;
    let mut slides: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .filter_map(|n| {
            let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
            Some((num, n))
        })
        .collect();
    slides.sort();
    let breaks = [("a:p", "", "\n"), ("a:br", "\n", ""), ("a:tab", "\t", "")];
    let mut out = String::new();
    let mut budget = MAX_ZIP_TOTAL;
    for (num, name) in &slides {
        if budget == 0 {
            break;
        }
        let xml = zip_entry(&mut zip, name, &mut budget)?.unwrap_or_default();
        out.push_str(&format!("--- Slide {num} ---\n{}\n\n", tidy(&office_xml_text(&xml, "a:t", &breaks))));
        let notes = format!("ppt/notesSlides/notesSlide{num}.xml");
        if let Some(xml) = zip_entry(&mut zip, &notes, &mut budget)? {
            let t = tidy(&office_xml_text(&xml, "a:t", &breaks));
            if !t.is_empty() {
                out.push_str(&format!("Notes: {t}\n\n"));
            }
        }
    }
    Ok((out, slides.len()))
}

fn spreadsheet_text(bytes: &[u8]) -> Result<(String, usize)> {
    use calamine::{Data, Reader};
    check_zip_total(bytes)?;
    let bytes = bytes.to_vec();
    let parsed = std::panic::catch_unwind(move || -> Result<(String, usize)> {
        let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| anyhow!("Couldn't open this spreadsheet: {e}"))?;
        let names = book.sheet_names();
        let mut out = String::new();
        for name in &names {
            let Ok(range) = book.worksheet_range(name) else { continue };
            out.push_str(&format!("--- Sheet: {name} ({} rows × {} columns) ---\n", range.height(), range.width()));
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        Data::DateTime(d) => d.as_datetime().map(|t| t.format("%Y-%m-%d %H:%M").to_string().trim_end_matches(" 00:00").to_string()).unwrap_or_else(|| c.to_string()),
                        Data::String(s) => s.replace(['\t', '\n', '\r'], " "),
                        other => other.to_string(),
                    })
                    .collect();
                if cells.iter().all(|c| c.is_empty()) {
                    continue;
                }
                out.push_str(cells.join("\t").trim_end());
                out.push('\n');
                if out.len() > MAX_ZIP_TOTAL as usize {
                    break;
                }
            }
            out.push('\n');
            if out.len() > MAX_ZIP_TOTAL as usize {
                break;
            }
        }
        Ok((out, names.len()))
    });
    match parsed {
        Ok(res) => res,
        Err(_) => Err(anyhow!("Couldn't read this spreadsheet.")),
    }
}

/// Readable text from HTML: no scripts or styles, line breaks at block ends.
pub fn html_text(html: &str) -> String {
    html_plain(html, false)
}

/// The value of attribute `name` in the inside of a tag (`a href="…" class="…"`).
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let rest = tag[from..].trim_start();
        if at == 0 || !lower.as_bytes()[at - 1].is_ascii_whitespace() || !rest.starts_with('=') {
            continue;
        }
        let v = rest[1..].trim_start();
        let val = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(char::is_whitespace).next().unwrap_or(""),
        };
        return Some(unescape_xml(val.trim()));
    }
    None
}

/// `html_text`, and with `links` also where links go and what images are (their alt text),
/// which a reader of mail needs: "click here" is useless without the address.
pub(crate) fn html_plain(html: &str, links: bool) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut skip_until: Option<&str> = None;
    // The link being read: its address and where its text starts in `out`.
    let mut link: Option<(String, usize)> = None;
    while let Some(lt) = rest.find('<') {
        if skip_until.is_none() {
            out.push_str(&unescape_xml(&rest[..lt]).replace(['\n', '\r'], " "));
        }
        // Comments can hold tags (Outlook's conditional blocks), so they end at "-->" only.
        if rest[lt..].starts_with("<!--") {
            let Some(end) = rest[lt..].find("-->") else { return finish(out) };
            rest = &rest[lt + end + 3..];
            continue;
        }
        let Some(gt) = rest[lt..].find('>') else {
            // A "<" that opens no tag is just text.
            if skip_until.is_none() {
                out.push_str(&unescape_xml(&rest[lt..]));
            }
            return finish(out);
        };
        let raw = &rest[lt + 1..lt + gt];
        let tag = raw.to_ascii_lowercase();
        rest = &rest[lt + gt + 1..];
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_string();
        if let Some(end) = skip_until {
            if closing && name == end {
                skip_until = None;
            }
            continue;
        }
        match name.as_str() {
            "script" if !closing => skip_until = Some("script"),
            "style" if !closing => skip_until = Some("style"),
            "head" if !closing => skip_until = Some("head"),
            "br" => out.push('\n'),
            "a" if links && !closing => {
                link = attr(raw, "href").filter(|h| h.starts_with("http://") || h.starts_with("https://")).map(|h| (h, out.len()));
            }
            "a" if links => {
                if let Some((href, start)) = link.take() {
                    let text = out[start..].trim();
                    if !text.is_empty() && !href.contains(text) && !text.contains(href.as_str()) {
                        out.push_str(&format!(" ({href})"));
                    }
                }
            }
            "img" if links => {
                if let Some(alt) = attr(raw, "alt").filter(|a| !a.trim().is_empty()) {
                    out.push_str(&format!("[image: {}]", alt.trim()));
                }
            }
            "td" | "th" if closing => out.push_str(" | "),
            "li" if !closing => out.push_str("\n• "),
            "p" | "div" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "blockquote" | "table" | "section" | "article" | "header" | "footer" => {
                out.push('\n')
            }
            _ => {}
        }
    }
    if skip_until.is_none() {
        out.push_str(&unescape_xml(rest));
    }
    finish(out)
}

fn finish(out: String) -> String {
    let lines: Vec<String> = out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    tidy(&lines.join("\n"))
}

fn email_text(bytes: &[u8]) -> String {
    use mail_parser::MessageParser;
    let Some(m) = MessageParser::default().parse(bytes) else {
        return String::from_utf8_lossy(bytes).into();
    };
    let addrs = |a: Option<&mail_parser::Address>| -> String {
        a.map(|a| {
            a.iter()
                .map(|x| match (&x.name, &x.address) {
                    (Some(n), Some(e)) => format!("{n} <{e}>"),
                    (_, Some(e)) => e.to_string(),
                    (Some(n), None) => n.to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
    };
    let mut out = format!(
        "From: {}\nTo: {}\nCc: {}\nDate: {}\nSubject: {}\n\n",
        addrs(m.from()),
        addrs(m.to()),
        addrs(m.cc()),
        m.date().map(|d| d.to_rfc3339()).unwrap_or_default(),
        m.subject().unwrap_or("")
    );
    let body = match m.body_text(0) {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => m.body_html(0).map(|h| html_text(&h)).unwrap_or_default(),
    };
    out.push_str(&tidy(&body));
    let names: Vec<String> = m.attachments().filter_map(|p| mail_parser::MimeHeaders::attachment_name(p).map(String::from)).collect();
    if !names.is_empty() {
        out.push_str(&format!("\n\nAttachments: {}", names.join(", ")));
    }
    out
}

fn zip_listing(bytes: &[u8]) -> Result<String> {
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow!("Couldn't open this zip file: {e}"))?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.to_string())).collect();
    Ok(format!("A zip archive with {} files (their contents can't be read):\n{}", names.len(), names.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in files {
                z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn word_and_slides() {
        let doc = r#"<w:document><w:body><w:p><w:r><w:t>Total &amp; tax</w:t></w:r><w:r><w:tab/><w:t xml:space="preserve">Rp 1.500.000</w:t></w:r></w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let text = docx_text(&zip_of(&[("word/document.xml", doc)])).unwrap();
        assert!(text.starts_with("Total & tax\tRp 1.500.000\n"), "{text}");
        assert!(text.ends_with("\nA | B |"), "{text}");

        let slide = |t: &str| format!(r#"<p:sld><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"#);
        let (s1, s2) = (slide("Roadmap"), slide("Hiring"));
        let (text, n) = pptx_text(&zip_of(&[("ppt/slides/slide2.xml", &s2), ("ppt/slides/slide1.xml", &s1)])).unwrap();
        assert_eq!(n, 2);
        assert_eq!(text, "--- Slide 1 ---\nRoadmap\n\n--- Slide 2 ---\nHiring\n\n");
    }

    #[test]
    fn html_and_entities() {
        let t = html_text("<html><head><title>x</title></head><body><style>p{}</style><p>Hi&nbsp;Budi,</p><table><tr><td>Item</td><td>Rp&#160;10</td></tr></table><script>alert(1)</script>Bye</body></html>");
        assert_eq!(t, "Hi Budi,\n\nItem | Rp 10 |\n\nBye");
        assert_eq!(unescape_xml("a &lt;b&gt; &#x41; & c"), "a <b> A & c");
    }

    #[test]
    fn links_images_and_comments() {
        let html = r#"<p>Pay <A class=x HREF="https://pay.example.com/?a=1&amp;b=2">here</a> or see <a href="https://example.com">https://example.com</a>.</p><!--[if mso]><v:rect>Button</v:rect><![endif]--><img src="x.png" alt="Logo"><a href="mailto:a@b.co">mail</a>"#;
        assert_eq!(html_plain(html, true), "Pay here (https://pay.example.com/?a=1&b=2) or see https://example.com.\n[image: Logo]mail");
        assert_eq!(html_text(html), "Pay here or see https://example.com.\nmail");
    }

    #[test]
    fn attachments_from_a_message() {
        let raw = b"From: a@example.com\r\nSubject: hi\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBody\r\n--b\r\nContent-Type: text/csv; name=data.csv\r\nContent-Disposition: attachment; filename=data.csv\r\n\r\nname,amount\r\nBudi,10\r\n--b\r\nContent-Type: image/png; name=x.png\r\nContent-Disposition: attachment; filename=x.png\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--b\r\nContent-Type: application/octet-stream; name=x.bin\r\nContent-Disposition: attachment; filename=x.bin\r\n\r\nzz\r\n--b--\r\n";
        match attachment_content(raw, 0, 0, 1000).unwrap() {
            AttachmentContent::Text { filename, text, total_chars, .. } => {
                assert_eq!(filename, "data.csv");
                assert_eq!(text, "name,amount\nBudi,10");
                assert_eq!(total_chars, 19);
            }
            other => panic!("{other:?}"),
        }
        match attachment_content(raw, 0, 12, 4).unwrap() {
            AttachmentContent::Text { text, offset, .. } => assert_eq!((text.as_str(), offset), ("Budi", 12)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(attachment_content(raw, 1, 0, 1000).unwrap(), AttachmentContent::Image { .. }));
        assert!(matches!(attachment_content(raw, 2, 0, 1000).unwrap(), AttachmentContent::Unsupported { .. }));
        assert!(attachment_content(raw, 3, 0, 1000).is_err());
    }
}
