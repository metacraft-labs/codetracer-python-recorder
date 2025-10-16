//! Selector parsing and matching utilities shared across scope and value filters.

use globset::{GlobBuilder, GlobMatcher};
use regex::Regex;
use recorder_errors::{usage, ErrorCode, RecorderResult};
use std::fmt;

/// Domains supported by the selector grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectorKind {
    Package,
    File,
    Object,
    Local,
    Global,
    Arg,
    Return,
    Attr,
}

impl SelectorKind {
    /// Return the token used in selector strings.
    pub fn token(self) -> &'static str {
        match self {
            SelectorKind::Package => "pkg",
            SelectorKind::File => "file",
            SelectorKind::Object => "obj",
            SelectorKind::Local => "local",
            SelectorKind::Global => "global",
            SelectorKind::Arg => "arg",
            SelectorKind::Return => "ret",
            SelectorKind::Attr => "attr",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token {
            "pkg" => Some(SelectorKind::Package),
            "file" => Some(SelectorKind::File),
            "obj" => Some(SelectorKind::Object),
            "local" => Some(SelectorKind::Local),
            "global" => Some(SelectorKind::Global),
            "arg" => Some(SelectorKind::Arg),
            "ret" => Some(SelectorKind::Return),
            "attr" => Some(SelectorKind::Attr),
            _ => None,
        }
    }

    /// Return true when the selector kind targets scope-level decisions.
    pub fn is_scope_kind(self) -> bool {
        matches!(
            self,
            SelectorKind::Package | SelectorKind::File | SelectorKind::Object
        )
    }

    /// Return true when the selector kind targets value-level decisions.
    pub fn is_value_kind(self) -> bool {
        !self.is_scope_kind()
    }
}

impl fmt::Display for SelectorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

/// Match strategy configured for a selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchType {
    Glob,
    Regex,
    Literal,
}

impl MatchType {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "glob" => Some(MatchType::Glob),
            "regex" => Some(MatchType::Regex),
            "literal" => Some(MatchType::Literal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum Matcher {
    Glob(GlobMatcher),
    Regex(Regex),
    Literal(String),
}

impl Matcher {
    fn matches(&self, candidate: &str) -> bool {
        match self {
            Matcher::Glob(matcher) => matcher.is_match(candidate),
            Matcher::Regex(regex) => regex.is_match(candidate),
            Matcher::Literal(expected) => candidate == expected,
        }
    }
}

/// Parsed selector with compiled matcher.
#[derive(Debug, Clone)]
pub struct Selector {
    raw: String,
    kind: SelectorKind,
    match_type: MatchType,
    pattern: String,
    matcher: Matcher,
}

impl Selector {
    /// Parse a selector string constrained to the provided kinds.
    ///
    /// When `permitted_kinds` is empty, all selector kinds are accepted.
    pub fn parse(raw: &str, permitted_kinds: &[SelectorKind]) -> RecorderResult<Self> {
        if raw.is_empty() {
            return Err(usage!(
                ErrorCode::InvalidPolicyValue,
                "selector string is empty"
            ));
        }

        let mut segments = raw.splitn(3, ':');
        let kind_token = segments
            .next()
            .ok_or_else(|| usage!(ErrorCode::InvalidPolicyValue, "selector must include a kind"))?;
        let remainder = segments
            .next()
            .ok_or_else(|| usage!(ErrorCode::InvalidPolicyValue, "selector missing pattern"))?;

        let kind = SelectorKind::parse(kind_token).ok_or_else(|| {
            usage!(
                ErrorCode::InvalidPolicyValue,
                "unsupported selector kind '{}'",
                kind_token
            )
        })?;

        if !permitted_kinds.is_empty() && !permitted_kinds.contains(&kind) {
            return Err(usage!(
                ErrorCode::InvalidPolicyValue,
                "selector kind '{}' is not allowed in this context",
                kind
            ));
        }

        let (match_type, pattern) = match segments.next() {
            Some(pattern) => {
                let match_token = remainder;
                if match_token.is_empty() {
                    return Err(usage!(
                        ErrorCode::InvalidPolicyValue,
                        "selector match type cannot be empty"
                    ));
                }
                let resolved_match =
                    MatchType::parse(match_token).ok_or_else(|| {
                        usage!(
                            ErrorCode::InvalidPolicyValue,
                            "unsupported selector match type '{}'",
                            match_token
                        )
                    })?;
                (resolved_match, pattern)
            }
            None => (MatchType::Glob, remainder),
        };

        if pattern.is_empty() {
            return Err(usage!(
                ErrorCode::InvalidPolicyValue,
                "selector pattern cannot be empty"
            ));
        }

        let matcher = build_matcher(match_type, pattern)?;
        Ok(Selector {
            raw: raw.to_string(),
            kind,
            match_type,
            pattern: pattern.to_string(),
            matcher,
        })
    }

    /// Selector kind.
    pub fn kind(&self) -> SelectorKind {
        self.kind
    }

    /// Match strategy.
    pub fn match_type(&self) -> MatchType {
        self.match_type
    }

    /// Raw pattern string (without kind/match prefix).
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Original selector string.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Evaluate whether the selector matches `candidate`.
    pub fn matches(&self, candidate: &str) -> bool {
        self.matcher.matches(candidate)
    }
}

fn build_matcher(match_type: MatchType, pattern: &str) -> RecorderResult<Matcher> {
    match match_type {
        MatchType::Literal => Ok(Matcher::Literal(pattern.to_string())),
        MatchType::Glob => {
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|err| {
                    usage!(
                        ErrorCode::InvalidPolicyValue,
                        "invalid glob pattern '{}': {}",
                        pattern,
                        err
                    )
                })?;
            Ok(Matcher::Glob(glob.compile_matcher()))
        }
        MatchType::Regex => {
            let regex = Regex::new(pattern).map_err(|err| {
                usage!(
                    ErrorCode::InvalidPolicyValue,
                    "invalid regex pattern '{}': {}",
                    pattern,
                    err
                )
            })?;
            Ok(Matcher::Regex(regex))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_parse(raw: &str, expected_kind: SelectorKind, mt: MatchType, pattern: &str) -> Selector {
        let selector = Selector::parse(raw, &[]).unwrap_or_else(|err| {
            panic!("selector parse failed for '{}': {}", raw, err);
        });
        assert_eq!(selector.kind(), expected_kind);
        assert_eq!(selector.match_type(), mt);
        assert_eq!(selector.pattern(), pattern);
        selector
    }

    #[test]
    fn parses_default_glob_scope_selector() {
        let selector = assert_parse("pkg:my_app.core.*", SelectorKind::Package, MatchType::Glob, "my_app.core.*");
        assert!(selector.matches("my_app.core.services"));
        assert!(!selector.matches("other.module"));
    }

    #[test]
    fn parses_literal_selector() {
        let selector = assert_parse(
            "file:literal:src/services/api.py",
            SelectorKind::File,
            MatchType::Literal,
            "src/services/api.py",
        );
        assert!(selector.matches("src/services/api.py"));
        assert!(!selector.matches("src/services/API.py"));
    }

    #[test]
    fn parses_regex_selector_with_colons() {
        let selector = assert_parse(
            "obj:regex:^my_app::service::[A-Z]\\w+$",
            SelectorKind::Object,
            MatchType::Regex,
            "^my_app::service::[A-Z]\\w+$",
        );
        assert!(selector.matches("my_app::service::Handler"));
        assert!(!selector.matches("my_app.service.Handler"));
    }

    #[test]
    fn rejects_unknown_kind() {
        let err = Selector::parse("unknown:foo", &[]).expect_err("expected kind error");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn rejects_disallowed_kind() {
        let err = Selector::parse("pkg:foo", &[SelectorKind::Local]).expect_err("kind not permitted");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn rejects_unknown_match_type() {
        let err = Selector::parse("pkg:invalid:foo", &[]).expect_err("expected match type error");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn rejects_empty_pattern() {
        let err = Selector::parse("pkg:", &[]).expect_err("expected empty pattern error");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn rejects_empty_string() {
        let err = Selector::parse("", &[]).expect_err("expected empty string error");
        assert_eq!(err.code, ErrorCode::InvalidPolicyValue);
    }

    #[test]
    fn matches_glob_against_values() {
        let selector = assert_parse("local:user_*", SelectorKind::Local, MatchType::Glob, "user_*");
        assert!(selector.matches("user_id"));
        assert!(!selector.matches("order_id"));
    }
}
