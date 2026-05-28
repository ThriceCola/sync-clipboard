//! Platform-abstracted clipboard operations.
//!
//! Each platform provides three functions behind a common interface:
//! - `read_once()` - read current clipboard (one-shot)
//! - `set_content()` - write to clipboard
//! - `start_monitor()` - spawn a background thread that sends changes via channel

use crate::message::ClipboardContent;
use std::sync::mpsc::Sender;

// ---- Linux (Wayland) implementation ----

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::thread;
    use wayland_clipboard_listener::{
        ClipBoardListenContext, WlClipboardCopyStream, WlClipboardPasteStream, WlListenType,
    };

    const TEXT_MIMES: &[&str] = &[
        "UTF8_STRING",
        "TEXT",
        "STRING",
        "text/plain",
        "text/plain;charset=utf-8",
    ];

    fn is_text_mime(mime: &str) -> bool {
        mime.starts_with("text/")
            || TEXT_MIMES.contains(&mime)
            || mime == "STRING"
            || mime == "TEXT"
            || mime == "UTF8_STRING"
    }

    fn context_to_content(ctx: ClipBoardListenContext) -> Option<ClipboardContent> {
        if ctx.context.is_empty() {
            return None;
        }
        if is_text_mime(&ctx.mime_type) {
            let text = String::from_utf8_lossy(&ctx.context).to_string();
            Some(ClipboardContent::Text(text))
        } else {
            Some(ClipboardContent::Image {
                mime_type: ctx.mime_type,
                data: ctx.context,
            })
        }
    }

    fn content_is_empty(content: &ClipboardContent) -> bool {
        match content {
            ClipboardContent::Text(s) => s.is_empty(),
            ClipboardContent::Image { data, .. } => data.is_empty(),
        }
    }

    pub fn set_content(content: ClipboardContent) {
        if content_is_empty(&content) {
            return;
        }
        thread::spawn(move || {
            let mut stream = match WlClipboardCopyStream::init() {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Failed to init copy stream: {e}");
                    return;
                }
            };

            let result = match &content {
                ClipboardContent::Text(s) => {
                    let data = s.as_bytes().to_vec();
                    stream.copy_to_clipboard(
                        data,
                        vec!["UTF8_STRING", "TEXT", "STRING", "text/plain"],
                        false,
                    )
                }
                ClipboardContent::Image { mime_type, data } => {
                    stream.copy_to_clipboard(data.clone(), vec![mime_type.as_str()], false)
                }
            };

            if let Err(e) = result {
                log::error!("Failed to set clipboard: {e}");
            }
        });
    }

    pub fn start_monitor(sender: Sender<ClipboardContent>) {
        thread::spawn(move || {
            let mut stream = match WlClipboardPasteStream::init(WlListenType::ListenOnCopy) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Failed to init Wayland clipboard listener: {e}");
                    return;
                }
            };

            log::info!("Wayland clipboard monitor started");

            for msg in stream.paste_stream().flatten() {
                if let Some(content) = context_to_content(msg.context) {
                    log::debug!("Clipboard changed: {:?}", content);
                    if sender.send(content).is_err() {
                        log::info!("Clipboard monitor channel closed, exiting");
                        break;
                    }
                }
            }
        });
    }

    /// Read content from clipboard once (used for dedup after remote set).
    pub fn read_once() -> Option<ClipboardContent> {
        let mut stream = WlClipboardPasteStream::init(WlListenType::ListenOnCopy).ok()?;
        let msg = stream.get_clipboard().ok()?;
        context_to_content(msg.context)
    }
}

// ---- Windows implementation ----

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use clipboard_win::{Clipboard, Monitor, Setter, formats, get_clipboard, set_clipboard_string};
    use image::ExtendedColorType;
    use image::codecs::bmp::BmpEncoder;
    use std::thread;

    fn content_is_empty(content: &ClipboardContent) -> bool {
        match content {
            ClipboardContent::Text(s) => s.is_empty(),
            ClipboardContent::Image { data, .. } => data.is_empty(),
        }
    }

    /// Convert arbitrary image bytes (PNG, JPEG, etc.) to a complete BMP file.
    /// `set_bitmap` expects a full BMP (BITMAPFILEHEADER + BITMAPINFOHEADER + pixels),
    /// which it uses to create an HBITMAP via `CreateDIBitmap`.
    ///
    /// NOTE: We encode as 24-bit RGB (`BI_RGB`), not 32-bit RGBA.
    /// `clipboard-win`'s `set_bitmap` copies only a fixed 40-byte `BITMAPINFOHEADER`
    /// to pass as `BITMAPINFO*` to `CreateDIBitmap`.  32-bit RGBA would produce
    /// `BI_BITFIELDS` headers with color masks after the info header, which
    /// `set_bitmap` does not include, causing `CreateDIBitmap` to fail.
    fn convert_to_bmp(data: &[u8]) -> Option<Vec<u8>> {
        let img = image::load_from_memory(data).ok()?;
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        let mut bmp = Vec::new();
        {
            let mut enc = BmpEncoder::new(&mut bmp);
            enc.encode(&rgb, w, h, ExtendedColorType::Rgb8).ok()?;
        }

        (bmp.len() > 14).then_some(bmp)
    }

    /// Read clipboard using `get_clipboard` which handles open/close internally.
    pub fn get_content() -> Option<ClipboardContent> {
        if let Ok(text) = get_clipboard::<String, _>(formats::Unicode) {
            if !text.is_empty() {
                return Some(ClipboardContent::Text(text));
            }
        }
        // CF_BITMAP: HBITMAP → BMP (handled by clipboard-win's `get_bitmap`)
        if let Ok(data) = get_clipboard::<Vec<u8>, _>(formats::Bitmap) {
            if !data.is_empty() {
                return Some(ClipboardContent::Image {
                    mime_type: "image/bmp".into(),
                    data,
                });
            }
        }
        // CF_DIB (8): raw DIB data (BITMAPINFOHEADER + pixels).
        // Some apps put images here exclusively. Prepend a BMP file header so
        // the wire format stays consistent.
        if let Ok(mut data) = get_clipboard::<Vec<u8>, _>(formats::RawData(8)) {
            if !data.is_empty() {
                // DIB = BITMAPINFOHEADER + pixels.
                // BMP file header: "BM" + file_size + reserved(4) + data_offset(54).
                let file_size = (14 + data.len()) as u32;
                let mut bmp = Vec::with_capacity(14 + data.len());
                bmp.extend_from_slice(b"BM");
                bmp.extend_from_slice(&file_size.to_le_bytes());
                bmp.extend_from_slice(&[0u8; 4]); // reserved
                bmp.extend_from_slice(&54u32.to_le_bytes()); // bfOffBits
                bmp.append(&mut data);
                return Some(ClipboardContent::Image {
                    mime_type: "image/bmp".into(),
                    data: bmp,
                });
            }
        }
        None
    }

    pub fn set_content(content: ClipboardContent) {
        if content_is_empty(&content) {
            return;
        }
        match content {
            ClipboardContent::Text(s) => {
                if let Err(e) = set_clipboard_string(&s) {
                    log::error!("Failed to set text clipboard: {e}");
                }
            }
            ClipboardContent::Image { mime_type, data } => {
                // `new_attempts` opens the clipboard; dropping `_clip` closes it.
                let _clip = Clipboard::new_attempts(10);

                // Try direct write first — works when `data` is already a valid BMP
                // (e.g. from another Windows host).
                if formats::Bitmap.write_clipboard(&data).is_err() {
                    log::debug!(
                        "Direct bitmap write failed, attempting format conversion ({} -> BMP)",
                        mime_type
                    );
                    // `data` is raw image bytes (PNG, JPEG, etc.) from Linux.
                    // Decode and re-encode as a complete BMP for `set_bitmap`.
                    match convert_to_bmp(&data) {
                        Some(bmp) => {
                            if let Err(e) = formats::Bitmap.write_clipboard(&bmp) {
                                log::error!("Converted BMP write also failed: {e}");
                            } else {
                                log::debug!("Converted image to BMP ({} bytes)", bmp.len());
                            }
                        }
                        None => {
                            log::error!("Could not decode image, registering raw format");
                            if let Some(fid) = clipboard_win::register_format(&mime_type) {
                                let _ = formats::RawData(fid.get()).write_clipboard(&data);
                            }
                        }
                    }
                }
                // `_clip` dropped here → clipboard closed.
                log::debug!("Set image clipboard ({} bytes)", data.len());
            }
        }
    }

    pub fn start_monitor(sender: Sender<ClipboardContent>) {
        thread::spawn(move || {
            let mut monitor = match Monitor::new() {
                Ok(m) => m,
                Err(e) => {
                    log::error!("Failed to create clipboard monitor: {e}");
                    return;
                }
            };

            log::info!("Windows clipboard monitor started");

            loop {
                match monitor.recv() {
                    Ok(true) => {
                        if let Some(content) = get_content() {
                            log::debug!("Clipboard changed: {:?}", content);
                            if sender.send(content).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(false) => break,
                    Err(e) => {
                        log::error!("Clipboard monitor error: {e}");
                        break;
                    }
                }
            }
        });
    }

    /// Read content from clipboard once.
    pub fn read_once() -> Option<ClipboardContent> {
        get_content()
    }
}

// ---- Common interface re-exports ----

pub use platform::{read_once, set_content, start_monitor};
