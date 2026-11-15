//! Colors for the dark and light themes, overridable in theme.ini.

use std::fmt::Write;

use crate::ini;

pub const FILE: &str = "theme.ini";

/// 0xRRGGBBAA.
pub type Rgba = u32;

macro_rules! theme {
    ($( $field:ident : $dark:expr, $light:expr, $doc:expr; )*) => {
        #[derive(Clone, Debug)]
        pub struct Theme {
            pub dark: bool,
            $( pub $field: Rgba, )*
        }

        impl Theme {
            pub fn dark() -> Theme {
                Theme { dark: true, $( $field: $dark, )* }
            }

            pub fn light() -> Theme {
                Theme { dark: false, $( $field: $light, )* }
            }

            fn set(&mut self, key: &str, v: Rgba) -> bool {
                match key {
                    $( stringify!($field) => { self.$field = v; true } )*
                    _ => false,
                }
            }

            fn entries(&self) -> Vec<(&'static str, Rgba, &'static str)> {
                vec![ $( (stringify!($field), self.$field, $doc), )* ]
            }
        }
    };
}

theme! {
    bg:            0x16181dff, 0xf5f6f8ff, "window background";
    panel:         0x1e2128ff, 0xffffffff, "summary tiles, table headers and the status bar";
    border:        0x2c313aff, 0xd9dde3ff, "lines between areas";
    text:          0xd7dae0ff, 0x1f2328ff, "normal text";
    text_dim:      0x7f8794ff, 0x6a737dff, "labels and zero values";
    accent:        0x4c9affff, 0x0a64d8ff, "focus rings and highlights";
    row_alt:       0x1a1d22ff, 0xf0f2f5ff, "every other table row";
    row_hover:     0x22262eff, 0xe8ebefff, "the row under the mouse";
    sel:           0x264f78ff, 0xcce0ffff, "selected row in the focused table";
    sel_inactive:  0x2c313aff, 0xe1e4e8ff, "selected row in other tables";
    sel_text:      0xffffffff, 0x0b1f3aff, "text of the selected row in the focused table";
    input_bg:      0x0f1114ff, 0xffffffff, "search and palette input";
    input_border:  0x343a45ff, 0xc4c9d0ff, "search box border";
    selection:     0x3a6ea5ff, 0xb3d4ffff, "selected text in inputs";
    overlay:       0x00000080, 0x00000040, "shade behind the palette and dialogs";
    popup:         0x22262eff, 0xffffffff, "palette and dialog background";
    scrollbar:     0x3a404cff, 0xc4c9d0ff, "scrollbar thumb";
    cpu:           0x4c9affff, 0x0a64d8ff, "CPU graph";
    mem:           0xb580ffff, 0x8a3ffcff, "memory graph";
    disk:          0x3fbf7fff, 0x198038ff, "disk graph";
    net_recv:      0x2ec4d6ff, 0x0089a8ff, "network receive graph";
    net_send:      0xff9f43ff, 0xd35400ff, "network send graph";
    gpu:           0xff6b8bff, 0xd02670ff, "GPU graph";
    heat:          0xffb020ff, 0xf1a800ff, "CPU and GPU bars in the tables";
    danger:        0xff5c5cff, 0xd92d20ff, "end-process prompts and errors";
    ok:            0x3fbf7fff, 0x198038ff, "success messages";
}

pub fn parse_color(s: &str) -> Option<Rgba> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some((v << 8) | 0xff),
        8 => Some(v),
        _ => None,
    }
}

/// The built-in theme with theme.ini's colors for it applied.
pub fn load(dark: bool) -> Theme {
    let mut t = if dark { Theme::dark() } else { Theme::light() };
    let want = if dark { "dark" } else { "light" };
    for (section, key, value) in ini::read(&ini::config_path(FILE)) {
        if section == want
            && let Some(c) = parse_color(&value)
        {
            t.set(&key, c);
        }
    }
    t
}

fn hex(c: Rgba) -> String {
    if c & 0xff == 0xff { format!("#{:06x}", c >> 8) } else { format!("#{c:08x}") }
}

/// theme.ini with every color of both themes, as currently in effect.
pub fn to_ini() -> String {
    let mut s = String::new();
    s.push_str("; winproc-gui colors. Values are #rrggbb or #rrggbbaa.\r\n");
    s.push_str("; Edit and save; the app reloads this file when it gets focus.\r\n");
    s.push_str("; Delete a line to go back to the built-in color.\r\n");
    for dark in [true, false] {
        let t = load(dark);
        let _ = write!(s, "\r\n[{}]\r\n", if dark { "dark" } else { "light" });
        for (k, v, doc) in t.entries() {
            let _ = write!(s, "{k:<14} = {:<10} ; {doc}\r\n", hex(v));
        }
    }
    s
}

/// Whether Windows is set to dark mode for apps.
pub fn system_is_dark() -> bool {
    use windows::Win32::System::Registry::*;
    use windows::core::w;
    let mut v: u32 = 1;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as _),
            Some(&mut size),
        )
    };
    r.is_ok() && v == 0
}
