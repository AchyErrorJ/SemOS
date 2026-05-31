//! A simple markdown parser that can write formatted text to the terminal
//!
//! Entrypoint is `MdStream::parse_str(...)`

use alloc::vec::Vec;
use semos_std::io;

mod parse;
mod term;

/// An AST representation of a Markdown document
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MdStream<'a>(Vec<MdTree<'a>>);

impl<'a> MdStream<'a> {
    /// Parse a markdown string to a tokenstream
    #[must_use]
    pub fn parse_str(s: &str) -> MdStream<'_> {
        parse::entrypoint(s)
    }

    /// Write formatted output to a stdout buffer, optionally with
    /// a formatter for code blocks
    pub fn write_anstream_buf(
        &self,
        buf: &mut Vec<u8>,
        formatter: Option<&(dyn Fn(&str, &mut Vec<u8>) -> io::Result<()> + 'static)>,
    ) -> io::Result<()> {
        term::entrypoint(self, buf, formatter)
    }
}

/// Create an anstream buffer with the `Always` color choice
///
/// M27 R4 B5 TODO(Phase 2b): `anstream::Stdout::always` requires a
/// `std::io::Stdout` handle. semos_std exposes a unit `Stdout` struct
/// that impls `io::Write`, not the `std::io::Stdout` type anstream
/// expects. Until the vendored anstream is patched to accept a generic
/// `Write` (the no_std-friendly path), this constructor is only valid on
/// host builds. Marked TODO so anstream's port can wire it correctly.
pub fn create_stdout_bufwtr() -> anstream::Stdout {
    // M27 R4 B5 TODO(Phase 2b): see fn-level note. The call below is the
    // original std::io::stdout()-shaped invocation, retained for the host
    // build. On the SemOS target it will fail to type-check until either
    // anstream is patched or this is gated to cfg(not(target_os = "none")).
    anstream::Stdout::always(std::io::stdout())
}

/// A single tokentree within a Markdown document
#[derive(Clone, Debug, PartialEq)]
pub enum MdTree<'a> {
    /// Leaf types
    Comment(&'a str),
    CodeBlock {
        txt: &'a str,
        lang: Option<&'a str>,
    },
    CodeInline(&'a str),
    Strong(&'a str),
    Emphasis(&'a str),
    Strikethrough(&'a str),
    PlainText(&'a str),
    /// [Foo](www.foo.com) or simple anchor <www.foo.com>
    Link {
        disp: &'a str,
        link: &'a str,
    },
    /// `[Foo link][ref]`
    RefLink {
        disp: &'a str,
        id: Option<&'a str>,
    },
    /// [ref]: www.foo.com
    LinkDef {
        id: &'a str,
        link: &'a str,
    },
    /// Break bewtween two paragraphs (double `\n`), not directly parsed but
    /// added later
    ParagraphBreak,
    /// Break bewtween two lines (single `\n`)
    LineBreak,
    HorizontalRule,
    Heading(u8, MdStream<'a>),
    OrderedListItem(u16, MdStream<'a>),
    UnorderedListItem(MdStream<'a>),
}

impl<'a> From<Vec<MdTree<'a>>> for MdStream<'a> {
    fn from(value: Vec<MdTree<'a>>) -> Self {
        Self(value)
    }
}
