use std::default::Default;

use crate::rendering::{AnsiColor, Attrs, Color, TokenColorScheme};
use crate::sexp::core::AtomKind;

pub struct ColorScheme {
    pub whitespace: TokenColorScheme,
    pub parens: TokenColorScheme,

    pub plain_atom: TokenColorScheme,
    pub atom_escape_sequence: TokenColorScheme,
    pub atom_invalid_escape_sequence: TokenColorScheme,

    pub record_key_atom: TokenColorScheme,
    pub constructor_atom: TokenColorScheme,
    pub number_atom: TokenColorScheme,
    pub bool_atom: TokenColorScheme,
    pub date_atom: TokenColorScheme,
    pub time_atom: TokenColorScheme,

    pub comment: TokenColorScheme,
    pub error: TokenColorScheme,
}

impl Default for ColorScheme {
    fn default() -> Self {
        let default = Attrs::default();
        let inverted = default.invert();
        let dimmed = Attrs {
            dimmed: true,
            ..default
        };
        let search_match = Attrs::from_fg(Color::Ansi(AnsiColor::Yellow)).invert();

        fn inverted_for_focus_with_default_search_match(attrs: Attrs) -> TokenColorScheme {
            let search_match = Attrs::from_fg(Color::Ansi(AnsiColor::Yellow)).invert();

            TokenColorScheme {
                normal: attrs,
                focused: attrs.invert(),
                search_match,
                focused_search_match: Attrs::default().invert(),
            }
        }

        ColorScheme {
            whitespace: TokenColorScheme {
                normal: default,
                focused: default,
                search_match,
                focused_search_match: inverted,
            },
            parens: TokenColorScheme {
                normal: dimmed,
                focused: Attrs {
                    bold: true,
                    ..default
                },
                search_match,
                focused_search_match: inverted,
            },

            plain_atom: inverted_for_focus_with_default_search_match(Attrs::from_ansi_fg(
                AnsiColor::BrightGreen,
            )),
            atom_escape_sequence: inverted_for_focus_with_default_search_match(
                Attrs::from_ansi_fg(AnsiColor::BrightYellow),
            ),
            atom_invalid_escape_sequence: inverted_for_focus_with_default_search_match(
                Attrs::from_ansi_fg(AnsiColor::BrightGreen),
            ),

            record_key_atom: inverted_for_focus_with_default_search_match(Attrs::from_ansi_fg(
                AnsiColor::BrightBlue,
            )),
            constructor_atom: inverted_for_focus_with_default_search_match(Attrs {
                bold: true,
                ..Attrs::from_ansi_fg(AnsiColor::BrightRed)
            }),
            number_atom: inverted_for_focus_with_default_search_match(Attrs::from_ansi_fg(
                AnsiColor::BrightMagenta,
            )),
            bool_atom: inverted_for_focus_with_default_search_match(Attrs::from_ansi_fg(
                AnsiColor::BrightYellow,
            )),
            date_atom: inverted_for_focus_with_default_search_match(Attrs::from_ansi_fg(
                AnsiColor::BrightMagenta,
            )),
            time_atom: inverted_for_focus_with_default_search_match(Attrs::from_ansi_fg(
                AnsiColor::BrightMagenta,
            )),

            comment: TokenColorScheme {
                normal: dimmed,
                focused: default,
                search_match,
                focused_search_match: inverted,
            },
            error: TokenColorScheme {
                normal: Attrs::new(Color::Default, Color::Ansi(AnsiColor::Red)),
                focused: Attrs::new(Color::Default, Color::Ansi(AnsiColor::BrightRed)),
                // We don't search in error messages
                search_match: default,
                focused_search_match: default,
            },
        }
    }
}

impl ColorScheme {
    pub fn for_atom_kind(&self, atom_kind: AtomKind) -> TokenColorScheme {
        match atom_kind {
            AtomKind::Constructor => self.constructor_atom,
            AtomKind::RecordKey => self.record_key_atom,
            AtomKind::Number => self.number_atom,
            AtomKind::Bool => self.bool_atom,
            AtomKind::Date => self.date_atom,
            AtomKind::Time => self.time_atom,
            AtomKind::StringifiedList | AtomKind::Plain => self.plain_atom,
        }
    }
}
