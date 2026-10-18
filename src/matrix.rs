use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::iter::Peekable;
use std::str::CharIndices;
use tracing::warn;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "RawMatrixDefinition")]
pub struct MatrixDefinition {
    /// Boolean expression for this matrix, as written in the config
    trigger: String,
    /// Parsed form of `trigger`
    expr: Expr,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatrixDefinition {
    trigger: String,
}

impl TryFrom<RawMatrixDefinition> for MatrixDefinition {
    type Error = String;

    fn try_from(raw: RawMatrixDefinition) -> Result<Self, Self::Error> {
        let expr = Expr::parse(&raw.trigger)
            .map_err(|e| format!("invalid matrix trigger \"{}\": {}", raw.trigger, e))?;
        Ok(Self {
            trigger: raw.trigger,
            expr,
        })
    }
}

impl MatrixDefinition {
    pub fn eval(&self, sources: &HashMap<String, bool>) -> bool {
        self.expr.eval(&|name| match sources.get(name) {
            Some(state) => *state,
            None => {
                warn!(
                    "Matrix trigger \"{}\" references unknown source '{}', treating it as off.",
                    self.trigger, name
                );
                false
            }
        })
    }
}

/// A boolean expression over source states.
#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Source(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    fn parse(input: &str) -> Result<Self, ParseError> {
        let mut parser = Parser {
            input,
            chars: input.char_indices().peekable(),
        };
        let expr = parser.parse_or()?;
        parser.skip_whitespace();
        match parser.chars.peek() {
            None => Ok(expr),
            Some(&(pos, c)) => Err(ParseError::new(pos, format!("unexpected '{c}'"))),
        }
    }

    fn eval(&self, lookup: &impl Fn(&str) -> bool) -> bool {
        match self {
            Expr::Source(name) => lookup(name),
            Expr::Not(e) => !e.eval(lookup),
            Expr::And(a, b) => a.eval(lookup) && b.eval(lookup),
            Expr::Or(a, b) => a.eval(lookup) || b.eval(lookup),
        }
    }
}

#[derive(Debug, PartialEq)]
struct ParseError {
    /// Byte offset into the expression
    pos: usize,
    msg: String,
}

impl ParseError {
    fn new(pos: usize, msg: impl Into<String>) -> Self {
        Self {
            pos,
            msg: msg.into(),
        }
    }
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at position {}", self.msg, self.pos)
    }
}

/// Recursive descent parser. Grammar, from lowest to highest precedence:
///
/// ```text
/// or      := and ("||" and)*
/// and     := unary ("&&" unary)*
/// unary   := "!" unary | primary
/// primary := "'" name "'" | "(" or ")"
/// ```
struct Parser<'a> {
    input: &'a str,
    chars: Peekable<CharIndices<'a>>,
}

impl<'a> Parser<'a> {
    fn skip_whitespace(&mut self) {
        while self.chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
    }

    /// Position of the next character, or the end of input.
    fn pos(&mut self) -> usize {
        self.chars.peek().map_or(self.input.len(), |&(pos, _)| pos)
    }

    /// Consumes the two-character operator `op` (e.g. `&&`) if it comes next.
    fn eat_operator(&mut self, op: char) -> Result<bool, ParseError> {
        self.skip_whitespace();
        let pos = self.pos();
        if self.chars.next_if(|&(_, c)| c == op).is_none() {
            return Ok(false);
        }
        if self.chars.next_if(|&(_, c)| c == op).is_none() {
            return Err(ParseError::new(pos, format!("expected '{op}{op}'")));
        }
        Ok(true)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.eat_operator('|')? {
            let rhs = self.parse_and()?;
            lhs = Expr::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        while self.eat_operator('&')? {
            let rhs = self.parse_unary()?;
            lhs = Expr::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        if self.chars.next_if(|&(_, c)| c == '!').is_some() {
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        match self.chars.next() {
            Some((start, '\'')) => {
                let name_start = start + 1;
                for (pos, c) in self.chars.by_ref() {
                    if c == '\'' {
                        if pos == name_start {
                            return Err(ParseError::new(start, "empty source name"));
                        }
                        return Ok(Expr::Source(self.input[name_start..pos].to_string()));
                    }
                }
                Err(ParseError::new(start, "unterminated source name"))
            }
            Some((start, '(')) => {
                let expr = self.parse_or()?;
                self.skip_whitespace();
                match self.chars.next() {
                    Some((_, ')')) => Ok(expr),
                    Some((pos, c)) => {
                        Err(ParseError::new(pos, format!("expected ')', found '{c}'")))
                    }
                    None => Err(ParseError::new(start, "unclosed '('")),
                }
            }
            Some((pos, c)) => Err(ParseError::new(
                pos,
                format!("expected a quoted source name, '(' or '!', found '{c}'"),
            )),
            None => Err(ParseError::new(
                self.input.len(),
                "unexpected end of expression",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(expr: &str, sources: &[(&str, bool)]) -> bool {
        let sources = sources
            .iter()
            .map(|(name, state)| (name.to_string(), *state))
            .collect();
        MatrixDefinition::try_from(RawMatrixDefinition {
            trigger: expr.to_string(),
        })
        .unwrap()
        .eval(&sources)
    }

    fn parse_err(expr: &str) -> ParseError {
        Expr::parse(expr).expect_err(expr)
    }

    #[test]
    fn single_source() {
        assert!(eval("'a'", &[("a", true)]));
        assert!(!eval("'a'", &[("a", false)]));
    }

    #[test]
    fn missing_source_is_off() {
        assert!(!eval("'missing'", &[("a", true)]));
        assert!(eval("!'missing'", &[("a", true)]));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let s = [("a", true), ("b", false), ("c", false)];
        assert!(eval("'a' || 'b' && 'c'", &s));
        assert!(eval("'b' && 'c' || 'a'", &s));
        assert_eq!(
            Expr::parse("'a' || 'b' && 'c'").unwrap(),
            Expr::Or(
                Box::new(Expr::Source("a".into())),
                Box::new(Expr::And(
                    Box::new(Expr::Source("b".into())),
                    Box::new(Expr::Source("c".into()))
                ))
            )
        );
    }

    #[test]
    fn parentheses_override_precedence() {
        let s = [("a", true), ("b", false), ("c", false)];
        assert!(!eval("('a' || 'b') && 'c'", &s));
        assert!(eval("((('a')))", &s));
    }

    #[test]
    fn not_binds_tighter_than_and() {
        let expr = "!'Sunlight' && 'Awake'";
        assert!(eval(expr, &[("Sunlight", false), ("Awake", true)]));
        assert!(!eval(expr, &[("Sunlight", true), ("Awake", true)]));
        assert!(!eval(expr, &[("Sunlight", false), ("Awake", false)]));
        assert!(!eval(expr, &[("Sunlight", true), ("Awake", false)]));
    }

    #[test]
    fn not() {
        assert!(eval("!!'a'", &[("a", true)]));
        assert!(!eval("!('a' || 'b')", &[("a", false), ("b", true)]));
        assert!(eval("! ! ! 'a'", &[("a", false)]));
    }

    #[test]
    fn names_with_spaces_and_symbols() {
        let s = [("Steam Link", true), ("a&&b (x)", false)];
        assert!(eval("'Steam Link' && !'a&&b (x)'", &s));
    }

    #[test]
    fn whitespace() {
        let s = [("a", true), ("b", false)];
        assert!(eval("'a'&&!'b'", &s));
        assert!(eval("  'a'  &&\n\t!  ( 'b' )  ", &s));
    }

    #[test]
    fn parse_errors() {
        assert_eq!(parse_err("").pos, 0);
        assert_eq!(parse_err("   ").pos, 3);
        assert_eq!(parse_err("'a").pos, 0);
        assert_eq!(parse_err("'a' && ''").pos, 7);
        assert_eq!(parse_err("'a' & 'b'").pos, 4);
        assert_eq!(parse_err("'a' | 'b'").pos, 4);
        assert_eq!(parse_err("('a' && 'b'").pos, 0);
        assert_eq!(parse_err("('a' 'b')").pos, 5);
        assert_eq!(parse_err("'a')").pos, 3);
        assert_eq!(parse_err("'a' 'b'").pos, 4);
        assert_eq!(parse_err("'a' &&").pos, 6);
        assert_eq!(parse_err("|| 'a'").pos, 0);
        assert_eq!(parse_err("!").pos, 1);
        assert_eq!(parse_err("a").pos, 0);
        assert_eq!(parse_err("\"a\"").pos, 0);
    }

    #[derive(Deserialize)]
    struct TestConfig {
        matrix: HashMap<String, MatrixDefinition>,
    }

    fn deserialize(toml: &str) -> Result<TestConfig, config::ConfigError> {
        config::Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()?
            .try_deserialize()
    }

    #[test]
    fn deserialize_valid() {
        let cfg =
            deserialize("[matrix.day-night]\ntrigger = \"!'Sunlight' && 'Awake'\"\n").unwrap();
        let matrix = &cfg.matrix["day-night"];
        assert_eq!(matrix.trigger, "!'Sunlight' && 'Awake'");
        let sources = HashMap::from([("Sunlight".to_string(), false), ("Awake".to_string(), true)]);
        assert!(matrix.eval(&sources));
    }

    #[test]
    fn deserialize_invalid() {
        let err = deserialize("[matrix.x]\ntrigger = \"'a' &\"\n")
            .err()
            .unwrap();
        assert!(err.to_string().contains("invalid matrix trigger"), "{err}");
        assert!(deserialize("[matrix.x]\ntrigger = \"'a'\"\nfoo = 1\n").is_err());
    }
}
