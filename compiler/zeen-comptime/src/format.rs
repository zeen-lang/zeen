#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FormatChunk {
    Literal(String),
    Arg(FormatSpec),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatKind {
    Display,
    Debug,
    Hex { upper: bool },
    Oct,
    Bin,
    Float { precision: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FormatSpec {
    pub(crate) kind: FormatKind,
    pub(crate) width: Option<u32>,
    pub(crate) align: Option<Align>,
    pub(crate) zero: bool,
}

impl FormatSpec {
    fn plain(kind: FormatKind) -> Self {
        Self {
            kind,
            width: None,
            align: None,
            zero: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FormatParseError {
    UnclosedBrace { offset: usize },
    UnknownSpecifier { spec: String, offset: usize },
    InvalidPrecision { raw: String, offset: usize },
}

impl FormatParseError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::UnclosedBrace { .. } => "unclosed '{' in format string".to_string(),
            Self::UnknownSpecifier { spec, .. } => format!("unknown format specifier: '{spec}'"),
            Self::InvalidPrecision { raw, .. } => {
                format!("invalid precision '{raw}' in: `{{:.{raw}}}`")
            }
        }
    }
}

pub(crate) fn parse_format_string(input: &str) -> Result<Vec<FormatChunk>, FormatParseError> {
    let mut chunks = Vec::new();
    let mut literal = String::new();
    let mut chars = input.char_indices().peekable();

    while let Some((idx, chr)) = chars.next() {
        match chr {
            '{' => {
                if chars.peek().map(|(_, chr)| *chr) == Some('{') {
                    chars.next();
                    literal.push('{');
                    continue;
                }

                if !literal.is_empty() {
                    chunks.push(FormatChunk::Literal(std::mem::take(&mut literal)))
                }

                let mut spec_raw = String::new();
                let mut closed = false;

                for (_, chr) in chars.by_ref() {
                    if chr == '}' {
                        closed = true;
                        break;
                    }
                    spec_raw.push(chr);
                }

                if !closed {
                    return Err(FormatParseError::UnclosedBrace { offset: idx });
                }

                let spec = parse_spec(&spec_raw, idx)?;
                chunks.push(FormatChunk::Arg(spec));
            }

            '}' => {
                if chars.peek().map(|(_, c)| *c) == Some('}') {
                    chars.next();
                    literal.push('}');
                    continue;
                }

                literal.push('}');
            }

            _ => literal.push(chr),
        }
    }

    if !literal.is_empty() {
        chunks.push(FormatChunk::Literal(literal));
    }

    Ok(chunks)
}

fn parse_spec(inner: &str, offset: usize) -> Result<FormatSpec, FormatParseError> {
    let (head, tail) = match inner.split_once(':') {
        Some((head, tail)) => (head, Some(tail)),
        None => (inner, None),
    };

    let mut spec = FormatSpec::plain(FormatKind::Display);

    if !head.is_empty() {
        spec.kind = parse_kind(head, offset)?;
    }

    if let Some(tail) = tail {
        parse_padding(tail, &mut spec, offset)?;
    }

    Ok(spec)
}

fn parse_padding(raw: &str, spec: &mut FormatSpec, offset: usize) -> Result<(), FormatParseError> {
    let mut rest = raw;
    let chars: Vec<char> = rest.chars().collect();

    if chars.len() >= 2 && is_align(chars[1]) {
        if chars[0] != ' ' && chars[0] != '0' {
            return Err(FormatParseError::UnknownSpecifier {
                spec: raw.to_string(),
                offset,
            });
        }

        spec.align = Some(align_of(chars[1], raw, offset)?);
        spec.zero = chars[0] == '0';
        rest = &rest[chars[0].len_utf8() + 1..];
    } else if chars.first() == Some(&'0') && chars.len() > 1 {
        spec.zero = true;
        rest = &rest[1..];
    } else if chars.first().is_some_and(|c| is_align(*c)) {
        spec.align = Some(align_of(chars[0], raw, offset)?);
        rest = &rest[1..];
    }

    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    rest = &rest[digits.len()..];

    if !digits.is_empty() {
        match digits.parse::<u32>() {
            Ok(width) => spec.width = Some(width),
            Err(_) => {
                return Err(FormatParseError::UnknownSpecifier {
                    spec: raw.to_string(),
                    offset,
                });
            }
        }
    }

    if !rest.is_empty() {
        spec.kind = parse_kind(rest, offset)?;
    }

    Ok(())
}

fn parse_kind(raw: &str, offset: usize) -> Result<FormatKind, FormatParseError> {
    match raw {
        "?" => Ok(FormatKind::Debug),

        "hex" | "x" => Ok(FormatKind::Hex { upper: false }),
        "HEX" | "X" => Ok(FormatKind::Hex { upper: true }),
        "oct" | "o" => Ok(FormatKind::Oct),
        "bin" | "b" => Ok(FormatKind::Bin),

        s if s.starts_with('.') => {
            let digits = &s[1..];

            match digits.parse::<u32>() {
                Ok(n) => Ok(FormatKind::Float { precision: n }),
                Err(_) => Err(FormatParseError::InvalidPrecision {
                    raw: digits.to_string(),
                    offset,
                }),
            }
        }

        other => Err(FormatParseError::UnknownSpecifier {
            spec: other.to_string(),
            offset,
        }),
    }
}

fn is_align(chr: char) -> bool {
    matches!(chr, '<' | '>' | '^')
}

fn align_of(chr: char, raw: &str, offset: usize) -> Result<Align, FormatParseError> {
    match chr {
        '<' => Ok(Align::Left),
        '>' => Ok(Align::Right),
        _ => Err(FormatParseError::UnknownSpecifier {
            spec: raw.to_string(),
            offset,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_display_and_literals() {
        assert_eq!(
            parse_format_string("hello {}!").unwrap(),
            vec![
                FormatChunk::Literal("hello ".into()),
                FormatChunk::Arg(FormatSpec::plain(FormatKind::Display)),
                FormatChunk::Literal("!".into()),
            ]
        );
    }

    #[test]
    fn parses_debug() {
        assert_eq!(
            parse_format_string("{:?}").unwrap(),
            vec![FormatChunk::Arg(FormatSpec::plain(FormatKind::Debug))]
        );
    }

    #[test]
    fn parses_integer_specs() {
        let chunks = parse_format_string("{hex} {oct} {bin}").unwrap();

        assert_eq!(
            chunks,
            vec![
                FormatChunk::Arg(FormatSpec::plain(FormatKind::Hex { upper: false })),
                FormatChunk::Literal(" ".into()),
                FormatChunk::Arg(FormatSpec::plain(FormatKind::Oct)),
                FormatChunk::Literal(" ".into()),
                FormatChunk::Arg(FormatSpec::plain(FormatKind::Bin)),
            ]
        );
    }

    #[test]
    fn parses_float_precision() {
        assert_eq!(
            parse_format_string("{:.3}").unwrap(),
            vec![FormatChunk::Arg(FormatSpec::plain(FormatKind::Float {
                precision: 3
            }))]
        );
    }

    #[test]
    fn parses_width_alignment_and_zero_pad() {
        assert_eq!(
            parse_format_string("{:<8}{:08x}").unwrap(),
            vec![
                FormatChunk::Arg(FormatSpec {
                    kind: FormatKind::Display,
                    width: Some(8),
                    align: Some(Align::Left),
                    zero: false,
                }),
                FormatChunk::Arg(FormatSpec {
                    kind: FormatKind::Hex { upper: false },
                    width: Some(8),
                    align: None,
                    zero: true,
                }),
            ]
        );
    }

    #[test]
    fn parses_escaped_braces() {
        assert_eq!(
            parse_format_string("{{}}").unwrap(),
            vec![FormatChunk::Literal("{}".into())]
        );
    }

    #[test]
    fn rejects_unclosed_brace() {
        assert_eq!(
            parse_format_string("{"),
            Err(FormatParseError::UnclosedBrace { offset: 0 })
        );
    }

    #[test]
    fn rejects_unknown_specifier() {
        assert_eq!(
            parse_format_string("{:q}"),
            Err(FormatParseError::UnknownSpecifier {
                spec: "q".into(),
                offset: 0,
            })
        );
    }

    #[test]
    fn rejects_invalid_precision() {
        assert_eq!(
            parse_format_string("{:.x}"),
            Err(FormatParseError::InvalidPrecision {
                raw: "x".into(),
                offset: 0,
            })
        );
    }
}
