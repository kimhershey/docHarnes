//! HTML → PDF through the platform webview (design §3.4: converted office
//! documents are printed with WebView2 `PrintToPdf`; WebKitGTK's print
//! operation covers the Linux CI/dev build). A hidden window loads the HTML
//! bundle through the `doculens://print/<token>/` route, prints when the page
//! has finished loading, and is destroyed afterwards. Prints are serialised.

use std::path::Path;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use doculens_core::documents::convert::PdfPrinter;
use doculens_core::{CoreError, Result};

use crate::state::AppState;

const PRINT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct WebviewPdfPrinter {
    app: AppHandle,
    serial: Mutex<()>,
}

impl WebviewPdfPrinter {
    pub fn new(app: AppHandle) -> Self {
        Self { app, serial: Mutex::new(()) }
    }
}

fn print_url(token: &str) -> tauri::Url {
    // Tauri passes custom-protocol URLs through verbatim, so the platform
    // spelling (`http://doculens.localhost/…` on Windows) is built here.
    let s = format!("{}/print/{token}/index.html", crate::protocol::base_url());
    tauri::Url::parse(&s).expect("valid print url")
}

impl PdfPrinter for WebviewPdfPrinter {
    fn print_to_pdf(&self, html: &Path, pdf: &Path) -> Result<()> {
        let _guard = self.serial.lock().map_err(|_| CoreError::Convert("printer lock poisoned".into()))?;
        let dir = html.parent().ok_or_else(|| CoreError::Convert("html bundle has no parent".into()))?.to_path_buf();
        if let Some(parent) = pdf.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        let _ = std::fs::remove_file(pdf);
        let token = uuid::Uuid::new_v4().simple().to_string();
        let state = self.app.state::<AppState>();
        state
            .print_bundles
            .lock()
            .map_err(|_| CoreError::Convert("print registry poisoned".into()))?
            .insert(token.clone(), dir);
        let result = run_print(&self.app, &token, pdf);
        if let Ok(mut m) = state.print_bundles.lock() {
            m.remove(&token);
        }
        result
    }

    fn name(&self) -> &'static str {
        if cfg!(windows) {
            "webview2"
        } else if cfg!(target_os = "linux") {
            "webkitgtk"
        } else {
            "webview"
        }
    }
}

fn run_print(app: &AppHandle, token: &str, pdf: &Path) -> Result<()> {
    let (tx, rx) = mpsc::channel::<std::result::Result<(), String>>();
    let label = format!("print-{token}");
    let pdf_path = pdf.to_path_buf();
    let tx_load = Mutex::new(Some(tx));
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::CustomProtocol(print_url(token)))
        .title("DocuLens print")
        .inner_size(900.0, 1200.0)
        .visible(false)
        .skip_taskbar(true)
        .focused(false)
        .on_page_load(move |window, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            let Some(tx) = tx_load.lock().ok().and_then(|mut g| g.take()) else { return };
            let pdf_path = pdf_path.clone();
            let tx2 = tx.clone();
            let dispatched = window.with_webview(move |webview| platform::print(webview, &pdf_path, tx));
            if let Err(e) = dispatched {
                let _ = tx2.send(Err(format!("with_webview: {e}")));
            }
        });
    let window = builder.build().map_err(|e| CoreError::Convert(format!("print window: {e}")))?;
    let outcome = match rx.recv_timeout(PRINT_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(CoreError::Convert(format!("print failed: {e}"))),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CoreError::Timeout("print to pdf".into())),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CoreError::Convert("print window closed".into())),
    };
    let _ = window.destroy();
    outcome?;
    // The platform APIs report success before the file is fully flushed on
    // some systems; wait briefly for a non-empty file.
    for _ in 0..50 {
        if std::fs::metadata(pdf).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(CoreError::Convert("print produced no PDF file".into()))
}

#[cfg(target_os = "linux")]
mod platform {
    use std::cell::RefCell;
    use std::path::Path;
    use std::sync::mpsc::Sender;

    use webkit2gtk::{PrintOperation, PrintOperationExt};

    thread_local! {
        // Keep operations alive until their `finished` / `failed` signal fires.
        static ACTIVE: RefCell<Vec<PrintOperation>> = const { RefCell::new(Vec::new()) };
    }

    pub fn print(webview: tauri::webview::PlatformWebview, pdf: &Path, tx: Sender<Result<(), String>>) {
        let wv = webview.inner();
        let op = PrintOperation::new(&wv);
        let settings = gtk::PrintSettings::new();
        settings.set(gtk::PRINT_SETTINGS_OUTPUT_FILE_FORMAT.as_str(), Some("pdf"));
        settings.set(gtk::PRINT_SETTINGS_OUTPUT_URI.as_str(), Some(&format!("file://{}", pdf.display())));
        settings.set_printer("Print to File");
        settings.set_paper_size(&gtk::PaperSize::new(Some(gtk::PAPER_NAME_A4.as_str())));
        op.set_print_settings(&settings);
        let setup = gtk::PageSetup::new();
        setup.set_paper_size(&gtk::PaperSize::new(Some(gtk::PAPER_NAME_A4.as_str())));
        for setter in [
            gtk::PageSetup::set_top_margin,
            gtk::PageSetup::set_bottom_margin,
            gtk::PageSetup::set_left_margin,
            gtk::PageSetup::set_right_margin,
        ] {
            setter(&setup, 0.0, gtk::Unit::Mm);
        }
        op.set_page_setup(&setup);
        let tx_ok = tx.clone();
        op.connect_finished(move |o| {
            ACTIVE.with(|a| a.borrow_mut().retain(|x| x != o));
            let _ = tx_ok.send(Ok(()));
        });
        op.connect_failed(move |o, err| {
            ACTIVE.with(|a| a.borrow_mut().retain(|x| x != o));
            let _ = tx.send(Err(err.to_string()));
        });
        ACTIVE.with(|a| a.borrow_mut().push(op.clone()));
        op.print();
    }
}

#[cfg(windows)]
mod platform {
    use std::path::Path;
    use std::sync::mpsc::Sender;

    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment6, ICoreWebView2_7};
    use webview2_com::PrintToPdfCompletedHandler;
    use windows_core::{Interface, HSTRING, PCWSTR};

    pub fn print(webview: tauri::webview::PlatformWebview, pdf: &Path, tx: Sender<Result<(), String>>) {
        let result = (|| -> Result<(), String> {
            let controller = webview.controller();
            // A hidden window pauses rendering; printing still needs a live view.
            unsafe { controller.SetIsVisible(true) }.map_err(|e| e.to_string())?;
            let core = unsafe { controller.CoreWebView2() }.map_err(|e| e.to_string())?;
            let core7: ICoreWebView2_7 =
                core.cast().map_err(|e| format!("PrintToPdf needs WebView2 ≥ 1.0.992: {e}"))?;
            let settings = match webview.environment().cast::<ICoreWebView2Environment6>() {
                Ok(env6) => unsafe { env6.CreatePrintSettings() }.ok(),
                Err(_) => None,
            };
            if let Some(s) = &settings {
                unsafe {
                    // A4 in inches; margins come from the document's CSS `@page`.
                    let _ = s.SetPageWidth(8.27);
                    let _ = s.SetPageHeight(11.69);
                    let _ = s.SetMarginTop(0.0);
                    let _ = s.SetMarginBottom(0.0);
                    let _ = s.SetMarginLeft(0.0);
                    let _ = s.SetMarginRight(0.0);
                    let _ = s.SetShouldPrintBackgrounds(true);
                    let _ = s.SetScaleFactor(1.0);
                }
            }
            let path = HSTRING::from(pdf.as_os_str());
            let tx_done = tx.clone();
            let handler = PrintToPdfCompletedHandler::create(Box::new(move |hr, ok| {
                let _ = tx_done.send(if hr.is_ok() && ok.as_bool() {
                    Ok(())
                } else {
                    Err(format!("PrintToPdf reported failure ({hr:?})"))
                });
                Ok(())
            }));
            unsafe { core7.PrintToPdf(PCWSTR(path.as_ptr()), settings.as_ref(), &handler) }
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = tx.send(Err(e));
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use std::path::Path;
    use std::sync::mpsc::Sender;

    pub fn print(_webview: tauri::webview::PlatformWebview, _pdf: &Path, tx: Sender<Result<(), String>>) {
        let _ = tx.send(Err("printing is not supported on this platform".into()));
    }
}
