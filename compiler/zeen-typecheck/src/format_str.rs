/// Parsed format string chunk
#[derive(Debug, Clone, PartialEq)]
pub enum FormatChunk {
    /// Text segment without format
    Literal(String),
    /// Argument being formatted
    Arg(FormatSpec),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Formatting argument specifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    /// `{}` - Basic display. Argument must implement `Display` interface.
    Display,
    /// `{:?}` - Debug display. Argument must implement `Debug` interface.
    Debug,
    /// `{hex}` - Hexadecimal format. Argument must be integer.
    Hex { upper: bool },
    /// `{oct}` - Octal format. Argument must be integer.
    Oct,
    /// `{bin}` - Binary format. Argument must be integer.
    Bin,
    /// `{:.N}` - Float with specified N numbers after dot. Argument must be float.
    Float { precision: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSpec {
    pub kind: FormatKind,
    pub width: Option<u32>,
    pub align: Option<Align>,
    pub zero: bool,
}

impl FormatSpec {
    pub fn plain(kind: FormatKind) -> Self {
        Self {
            kind,
            width: None,
            align: None,
            zero: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormatParseError {
    UnclosedBrace { offset: usize },
    UnknownSpecifier { spec: String, offset: usize },
    InvalidPrecision { raw: String, offset: usize },
}

/// Parses a format string into `FormatChunk`s.
///
/// Supports Display, Debug, hex/oct/bin integer formats and fixed-precision
/// float formatting.
pub fn parse_format_string(input: &str) -> Result<Vec<FormatChunk>, FormatParseError> {
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

pub fn arg_specs(chunks: &[FormatChunk]) -> Vec<FormatSpec> {
    chunks
        .iter()
        .filter_map(|c| match c {
            FormatChunk::Arg(spec) => Some(*spec),
            FormatChunk::Literal(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display() {
        let chunks = parse_format_string("hello {}!").unwrap();
        assert_eq!(
            chunks,
            vec![
                FormatChunk::Literal("hello ".into()),
                FormatChunk::Arg(FormatSpec::plain(FormatKind::Display)),
                FormatChunk::Literal("!".into()),
            ]
        );
    }

    #[test]
    fn test_debug() {
        let chunks = parse_format_string("{:?}").unwrap();
        assert_eq!(
            chunks,
            vec![FormatChunk::Arg(FormatSpec::plain(FormatKind::Debug))]
        );
    }

    #[test]
    fn test_hex_oct_bin() {
        let chunks = parse_format_string("{hex} {oct} {bin}").unwrap();
        assert_eq!(
            arg_specs(&chunks),
            vec![
                FormatSpec::plain(FormatKind::Hex { upper: false }),
                FormatSpec::plain(FormatKind::Oct),
                FormatSpec::plain(FormatKind::Bin),
            ]
        );
    }

    #[test]
    fn test_short_integer_specs() {
        let chunks = parse_format_string("{:x}{:X}{:o}{:b}").unwrap();
        assert_eq!(
            arg_specs(&chunks),
            vec![
                FormatSpec::plain(FormatKind::Hex { upper: false }),
                FormatSpec::plain(FormatKind::Hex { upper: true }),
                FormatSpec::plain(FormatKind::Oct),
                FormatSpec::plain(FormatKind::Bin),
            ]
        );
    }

    #[test]
    fn test_float_precision() {
        let chunks = parse_format_string("{:.3}").unwrap();
        assert_eq!(
            chunks,
            vec![FormatChunk::Arg(FormatSpec::plain(FormatKind::Float {
                precision: 3
            }))]
        );
    }

    #[test]
    fn test_align_and_width() {
        let chunks = parse_format_string("{:<8}{:>8}{:^8}").unwrap_err();
        assert!(matches!(chunks, FormatParseError::UnknownSpecifier { .. }));

        let chunks = parse_format_string("{:<8}{:>8}").unwrap();

        assert_eq!(
            arg_specs(&chunks),
            vec![
                FormatSpec {
                    kind: FormatKind::Display,
                    width: Some(8),
                    align: Some(Align::Left),
                    zero: false,
                },
                FormatSpec {
                    kind: FormatKind::Display,
                    width: Some(8),
                    align: Some(Align::Right),
                    zero: false,
                },
            ]
        );
    }

    #[test]
    fn test_zero_pad_hex() {
        let chunks = parse_format_string("{:08x} {hex:04} {:#>6}").unwrap_err();
        assert!(matches!(chunks, FormatParseError::UnknownSpecifier { .. }));

        let chunks = parse_format_string("{:08x} {hex:04}").unwrap();

        assert_eq!(
            arg_specs(&chunks),
            vec![
                FormatSpec {
                    kind: FormatKind::Hex { upper: false },
                    width: Some(8),
                    align: None,
                    zero: true,
                },
                FormatSpec {
                    kind: FormatKind::Hex { upper: false },
                    width: Some(4),
                    align: None,
                    zero: true,
                },
            ]
        );
    }

    #[test]
    fn test_width_with_precision() {
        let chunks = parse_format_string("{:08.3}").unwrap();

        assert_eq!(
            arg_specs(&chunks),
            vec![FormatSpec {
                kind: FormatKind::Float { precision: 3 },
                width: Some(8),
                align: None,
                zero: true,
            }]
        );
    }

    #[test]
    fn test_escape_braces() {
        let chunks = parse_format_string("{{}}").unwrap();
        assert_eq!(chunks, vec![FormatChunk::Literal("{}".into()),]);
    }

    #[test]
    fn test_unclosed() {
        assert!(matches!(
            parse_format_string("{"),
            Err(FormatParseError::UnclosedBrace { .. })
        ));
    }

    #[test]
    fn test_unknown_spec() {
        assert!(matches!(
            parse_format_string("{:q}"),
            Err(FormatParseError::UnknownSpecifier { .. })
        ));
    }

    #[test]
    fn test_invalid_width() {
        assert!(matches!(
            parse_format_string("{:99999999999}"),
            Err(FormatParseError::UnknownSpecifier { .. })
        ));
    }

    #[test]
    fn test_invalid_precision() {
        assert!(matches!(
            parse_format_string("{:.x}"),
            Err(FormatParseError::InvalidPrecision { .. })
        ));
    }
}
