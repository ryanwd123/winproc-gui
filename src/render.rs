//! A thin drawing layer over Direct2D and DirectWrite. Everything is in DIPs.

use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{HSTRING, Result, w};
use windows_numerics::Vector2;

use crate::theme::Rgba;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Normal,
    Bold,
    Small,
    Big,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

fn color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c >> 24) & 0xff) as f32 / 255.0,
        g: ((c >> 16) & 0xff) as f32 / 255.0,
        b: ((c >> 8) & 0xff) as f32 / 255.0,
        a: (c & 0xff) as f32 / 255.0,
    }
}

/// `c` with its alpha multiplied by `a`.
pub fn alpha(c: Rgba, a: f32) -> Rgba {
    let base = (c & 0xff) as f32;
    (c & 0xffffff00) | ((base * a).clamp(0.0, 255.0) as u32)
}

pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    hwnd: HWND,
    hardware: bool,
    pub dpi: f32,
    pub w: f32,
    pub h: f32,
    pub size: f32,
    fonts: [IDWriteTextFormat; 4],
    wbuf: Vec<u16>,
}

fn make_format(dw: &IDWriteFactory, family: &str, size: f32, bold: bool) -> Result<IDWriteTextFormat> {
    unsafe {
        let f = dw.CreateTextFormat(
            &HSTRING::from(family),
            None,
            if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        if let Ok(sign) = dw.CreateEllipsisTrimmingSign(&f) {
            f.SetTrimming(&trim, &sign)?;
        }
        Ok(f)
    }
}

fn make_fonts(dw: &IDWriteFactory, family: &str, size: f32) -> Result<[IDWriteTextFormat; 4]> {
    Ok([
        make_format(dw, family, size, false)?,
        make_format(dw, family, size, true)?,
        make_format(dw, family, (size * 0.85).round().max(7.0), false)?,
        make_format(dw, family, (size * 1.55).round(), true)?,
    ])
}

impl Gfx {
    pub fn new(hwnd: HWND, family: &str, size: f32, hardware: bool, dpi: f32) -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = match make_fonts(&dw, family, size) {
                Ok(f) => f,
                Err(_) => make_fonts(&dw, "Segoe UI", size)?,
            };
            Ok(Gfx {
                d2d,
                dw,
                rt: None,
                brush: None,
                hwnd,
                hardware,
                dpi,
                w: 0.0,
                h: 0.0,
                size,
                fonts,
                wbuf: Vec::new(),
            })
        }
    }

    pub fn set_font(&mut self, family: &str, size: f32) {
        if let Ok(f) = make_fonts(&self.dw, family, size).or_else(|_| make_fonts(&self.dw, "Segoe UI", size)) {
            self.fonts = f;
            self.size = size;
        }
    }

    pub fn resize(&mut self, pw: u32, ph: u32, dpi: f32) {
        self.dpi = dpi;
        self.w = pw as f32 * 96.0 / dpi;
        self.h = ph as f32 * 96.0 / dpi;
        if let Some(rt) = &self.rt {
            unsafe {
                rt.SetDpi(dpi, dpi);
                let _ = rt.Resize(&D2D_SIZE_U { width: pw.max(1), height: ph.max(1) });
            }
        }
    }

    fn ensure_target(&mut self) -> Result<()> {
        if self.rt.is_some() {
            return Ok(());
        }
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: if self.hardware { D2D1_RENDER_TARGET_TYPE_DEFAULT } else { D2D1_RENDER_TARGET_TYPE_SOFTWARE },
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U {
                    width: (rc.right - rc.left).max(1) as u32,
                    height: (rc.bottom - rc.top).max(1) as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let rt = self.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            rt.SetDpi(self.dpi, self.dpi);
            self.brush = Some(rt.CreateSolidColorBrush(&color(0xffffffff), None)?);
            self.rt = Some(rt);
        }
        Ok(())
    }

    pub fn begin(&mut self, bg: Rgba) -> bool {
        if self.ensure_target().is_err() {
            return false;
        }
        let rt = self.rt.as_ref().unwrap();
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&color(bg)));
        }
        true
    }

    pub fn end(&mut self) {
        if let Some(rt) = &self.rt
            && let Err(e) = unsafe { rt.EndDraw(None, None) }
            && e.code() == D2DERR_RECREATE_TARGET
        {
            self.rt = None;
            self.brush = None;
        }
    }

    fn rt(&self) -> &ID2D1HwndRenderTarget {
        self.rt.as_ref().unwrap()
    }

    fn brush(&self, c: Rgba) -> &ID2D1SolidColorBrush {
        let b = self.brush.as_ref().unwrap();
        unsafe { b.SetColor(&color(c)) };
        b
    }

    pub fn fill(&self, r: Rect, c: Rgba) {
        unsafe { self.rt().FillRectangle(&r.d2d(), self.brush(c)) };
    }

    pub fn fill_round(&self, r: Rect, radius: f32, c: Rgba) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt().FillRoundedRectangle(&rr, self.brush(c)) };
    }

    pub fn stroke_round(&self, r: Rect, radius: f32, c: Rgba, width: f32) {
        let r = r.inset(width / 2.0, width / 2.0);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt().DrawRoundedRectangle(&rr, self.brush(c), width, None) };
    }

    pub fn hline(&self, x0: f32, x1: f32, y: f32, c: Rgba) {
        self.fill(Rect::new(x0, y, x1 - x0, 1.0), c);
    }

    pub fn vline(&self, x: f32, y0: f32, y1: f32, c: Rgba) {
        self.fill(Rect::new(x, y0, 1.0, y1 - y0), c);
    }

    pub fn clip(&self, r: Rect) {
        unsafe { self.rt().PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn unclip(&self) {
        unsafe { self.rt().PopAxisAlignedClip() };
    }

    fn format(&self, f: Font) -> &IDWriteTextFormat {
        &self.fonts[f as usize]
    }

    fn wide(&mut self, s: &str) {
        self.wbuf.clear();
        self.wbuf.extend(s.encode_utf16());
    }

    pub fn text(&mut self, s: &str, r: Rect, font: Font, c: Rgba, align: Align) {
        if s.is_empty() || r.w <= 0.0 {
            return;
        }
        self.wide(s);
        let f = self.format(font);
        unsafe {
            let _ = f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            });
            self.rt().DrawText(
                &self.wbuf,
                f,
                &r.d2d(),
                self.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn layout(&mut self, s: &str, font: Font) -> Option<IDWriteTextLayout> {
        self.wide(s);
        let f = self.format(font);
        unsafe {
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            self.dw.CreateTextLayout(&self.wbuf, f, 10000.0, 100.0).ok()
        }
    }

    /// Width of `s`, including trailing spaces.
    pub fn measure(&mut self, s: &str, font: Font) -> f32 {
        let Some(l) = self.layout(s, font) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = l.GetMetrics(&mut m);
        }
        m.widthIncludingTrailingWhitespace
    }

    /// The x offset of UTF-16 position `pos` in `s`.
    pub fn caret_x(&mut self, s: &str, font: Font, pos: u32) -> f32 {
        let Some(l) = self.layout(s, font) else { return 0.0 };
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = l.HitTestTextPosition(pos, false, &mut x, &mut y, &mut m);
        }
        x
    }

    /// The UTF-16 position nearest to `x` in `s`.
    pub fn hit(&mut self, s: &str, font: Font, x: f32) -> u32 {
        let Some(l) = self.layout(s, font) else { return 0 };
        let (mut trailing, mut inside) = (BOOL_FALSE, BOOL_FALSE);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = l.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m);
        }
        m.textPosition + if trailing.as_bool() { m.length } else { 0 }
    }

    /// A filled area under `values` (0..=1) spread across `r`, with a line on top.
    pub fn area(&self, r: Rect, values: &[f32], cap: usize, fill: Rgba, line: Rgba) {
        if values.len() < 2 {
            return;
        }
        let step = r.w / (cap.max(2) - 1) as f32;
        let x0 = r.right() - step * (values.len() - 1) as f32;
        let pts: Vec<Vector2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| Vector2 { X: x0 + step * i as f32, Y: r.bottom() - v.clamp(0.0, 1.0) * r.h })
            .collect();
        unsafe {
            if let Ok(g) = self.d2d.CreatePathGeometry()
                && let Ok(sink) = g.Open()
            {
                sink.BeginFigure(Vector2 { X: pts[0].X, Y: r.bottom() }, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&pts);
                sink.AddLine(Vector2 { X: pts[pts.len() - 1].X, Y: r.bottom() });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                let _ = sink.Close();
                self.rt().FillGeometry(&g, self.brush(fill), None);
            }
            if let Ok(g) = self.d2d.CreatePathGeometry()
                && let Ok(sink) = g.Open()
            {
                sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddLines(&pts[1..]);
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                let _ = sink.Close();
                self.rt().DrawGeometry(&g, self.brush(line), 1.2, None);
            }
        }
    }
}

const BOOL_FALSE: windows::core::BOOL = windows::core::BOOL(0);
