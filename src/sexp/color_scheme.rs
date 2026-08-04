use std::default::Default;

use crate::rendering::{AnsiColor, Attrs, Color, HighlightAttrs, TokenColorScheme};
use crate::sexp::core::AtomKind;

pub struct ColorScheme {
    pub default: Attrs,
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
    // Someday: This implementation feels pretty clunky / overly verbose.
    // How many distinct styles really need to be specified?
    fn default() -> Self {
        let default = Attrs::default();
        let inverted = default.invert();
        let dimmed = Attrs {
            dimmed: true,
            ..default
        };
        let dimmed_inverted = Attrs::from_fg(Color::Ansi(AnsiColor::BrightBlack)).invert();

        let current_search_match = inverted;
        let other_search_match = Attrs::from_fg(Color::Ansi(AnsiColor::Yellow)).invert();

        fn inverted_for_focus_with_default_highlight_attrs(attrs: Attrs) -> TokenColorScheme {
            let current_search_match = Attrs::default().invert();
            let other_search_match = Attrs::from_fg(Color::Ansi(AnsiColor::Yellow)).invert();

            TokenColorScheme {
                normal: HighlightAttrs {
                    not_a_match: attrs,
                    current_match: current_search_match,
                    other_match: other_search_match,
                },
                focused: HighlightAttrs {
                    not_a_match: attrs.invert(),
                    current_match: current_search_match,
                    other_match: other_search_match,
                },
            }
        }

        ColorScheme {
            default,
            whitespace: TokenColorScheme {
                normal: HighlightAttrs {
                    not_a_match: default,
                    current_match: current_search_match,
                    other_match: other_search_match,
                },
                focused: HighlightAttrs {
                    not_a_match: default,
                    current_match: current_search_match,
                    other_match: other_search_match,
                },
            },
            parens: TokenColorScheme {
                normal: HighlightAttrs {
                    not_a_match: dimmed,
                    current_match: current_search_match,
                    other_match: other_search_match,
                },
                focused: HighlightAttrs {
                    not_a_match: Attrs {
                        bold: true,
                        ..default
                    },
                    current_match: Attrs {
                        bold: true,
                        ..current_search_match
                    },
                    other_match: Attrs {
                        bold: true,
                        ..other_search_match
                    },
                },
            },

            plain_atom: inverted_for_focus_with_default_highlight_attrs(Attrs::from_ansi_fg(
                AnsiColor::BrightGreen,
            )),
            atom_escape_sequence: inverted_for_focus_with_default_highlight_attrs(
                Attrs::from_ansi_fg(AnsiColor::BrightYellow),
            ),
            atom_invalid_escape_sequence: inverted_for_focus_with_default_highlight_attrs(
                Attrs::from_ansi_fg(AnsiColor::BrightGreen),
            ),

            record_key_atom: inverted_for_focus_with_default_highlight_attrs(Attrs::from_ansi_fg(
                AnsiColor::BrightBlue,
            )),
            constructor_atom: inverted_for_focus_with_default_highlight_attrs(Attrs {
                bold: true,
                ..Attrs::from_ansi_fg(AnsiColor::BrightRed)
            }),
            number_atom: inverted_for_focus_with_default_highlight_attrs(Attrs::from_ansi_fg(
                AnsiColor::BrightMagenta,
            )),
            bool_atom: inverted_for_focus_with_default_highlight_attrs(Attrs::from_ansi_fg(
                AnsiColor::Yellow,
            )),
            date_atom: inverted_for_focus_with_default_highlight_attrs(Attrs::from_ansi_fg(
                AnsiColor::Cyan,
            )),
            time_atom: inverted_for_focus_with_default_highlight_attrs(Attrs::from_ansi_fg(
                AnsiColor::Cyan,
            )),

            comment: TokenColorScheme {
                normal: HighlightAttrs {
                    not_a_match: dimmed,
                    // It's impossible for the current match to be in a comment, and not
                    // have that comment be focused, but we actually also use comment styles
                    // for collapsed previews, so if you search into a collapsed container and
                    // the match is at the start, it will use this style, but we don't want to
                    // give the impression that that match is focused in any way, so we don't
                    // distinguish between current match and other match.
                    //
                    // Someday: Use separate styles for previews.
                    current_match: dimmed_inverted,
                    other_match: dimmed_inverted,
                },
                focused: HighlightAttrs {
                    not_a_match: default,
                    // Someday: This isn't very easy to distinguish from non-bold.
                    current_match: Attrs {
                        bold: true,
                        ..inverted
                    },
                    other_match: inverted,
                },
            },
            error: TokenColorScheme {
                // We don't search in error messages
                normal: HighlightAttrs {
                    not_a_match: Attrs::new(Color::Default, Color::Ansi(AnsiColor::Red)),
                    current_match: default,
                    other_match: default,
                },
                focused: HighlightAttrs {
                    not_a_match: Attrs::new(Color::Default, Color::Ansi(AnsiColor::BrightRed)),
                    current_match: default,
                    other_match: default,
                },
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
