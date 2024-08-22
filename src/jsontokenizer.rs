use logos::Logos;

// A basic JSON tokenizer

#[derive(Logos, Debug, Copy, Clone, PartialEq)]
pub enum JsonToken {
    // Characters
    #[token("{")]
    OpenCurly,
    #[token("}")]
    CloseCurly,
    #[token("[")]
    OpenSquare,
    #[token("]")]
    CloseSquare,
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token("null")]
    Null,
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[regex(r"-?(0|([1-9][0-9]*))(\.[0-9]+)?([eE][-+]?[0-9]+)?")]
    Number,
    // I get an error when I do [0-9a-fA-F]{4}.
    #[regex("\"((\\\\([\"\\\\/bfnrt]|u[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]))|[^\"\\\\\x00-\x1F])*\"")]
    String,

    // Whitespace; need separate newline token to handle newline-delimited JSON.
    #[token("\n")]
    Newline,
    #[regex("[ \t\r]+", logos::skip)]
    Whitespace,
}
