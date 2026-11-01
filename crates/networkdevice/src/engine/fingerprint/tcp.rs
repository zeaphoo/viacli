//! TCP service fingerprints: which service is behind that open port, and which
//! version of it.
//!
//! This is the `nmap-service-probes` format: a set of probes to send, and, per
//! probe, an ordered list of rules to test the answer against. A rule that
//! matches names the service and can pull the product, the version and a CPE out
//! of the response.
//!
//! The format is nmap's; the data here is **not**. nmap's probe file is nmap's
//! data, so it is not vendored — what ships is a small table written for this
//! project, covering the services a LAN actually runs. If you want nmap's far
//! larger table, point [`Engine::load`] at your own copy: the parser is the
//! same, and a supplied file's probes are tried before the built-in ones.
//!
//! Probe selection follows nmap's mechanism:
//!
//! * a port's own probes are tried before the generic ones, and each group is
//!   ordered by `rarity`, lowest first (an absent rarity sorts last);
//! * a probe that names only `sslports` is not sent in the clear;
//! * every probe carries a fallback chain — itself, the probes a `fallback`
//!   directive names, then `NULL` — so a response the sending probe cannot place
//!   is still offered to another probe's rules before giving up.
//!
//! The `m`/`s` patterns of the **built-in table** are this project's glob
//! dialect: `*` is a wildcard and also a capture, so `m|SSH-2.0-OpenSSH_*|` with
//! `v/$1/` yields the version without a regex, and a `*` never crosses a line
//! break.
//!
//! A file named by `probes_file` is read as nmap's own dialect instead: `m`/`s`
//! are regular expressions, anchored or not, and `r`/`r=` are too. That is what
//! makes nmap's full `nmap-service-probes` usable. Rules the Rust regex engine
//! cannot compile — look-around and back-references — are counted in
//! [`Engine::skipped_regex`], never silently swallowed.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use regex::Regex;

/// How much of a response we keep. Service banners are short; anything longer is
/// an HTTP body we do not need.
const RESPONSE_LIMIT: usize = 4096;
/// Probes tried on one port before giving up on it.
const PROBES_PER_PORT: usize = 3;
/// Backtracking guard for the pattern matcher, so a pathological pattern cannot
/// spin on a long response.
const MATCH_STEPS: u32 = 100_000;

/// A service identified on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceMatch {
    /// The service name: `http`, `ssh`, `redis`.
    pub service: String,
    pub product: Option<String>,
    pub version: Option<String>,
    pub info: Option<String>,
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub device_type: Option<String>,
    pub cpe: Option<String>,
    /// The response the match was made against, for the port banner.
    pub response: String,
}

impl ServiceMatch {
    /// `product version`, when either is known.
    pub fn describe(&self) -> Option<String> {
        match (&self.product, &self.version) {
            (Some(product), Some(version)) => Some(format!("{product} {version}")),
            (Some(product), None) => Some(product.clone()),
            (None, Some(version)) => Some(version.clone()),
            (None, None) => None,
        }
    }
}

// ------------------------------------------------------------- patterns

/// Which dialect the `m`/`s` patterns are written in.
///
/// The built-in table uses this project's glob dialect (`*` is a wildcard and
/// also a capture). A file that is actually nmap's own `nmap-service-probes`
/// uses nmap's dialect instead, where `m`/`s` mean "anchored" / "anywhere" and
/// the pattern itself is a regular expression. The two are not interchangeable,
/// so the dialect is chosen by where the text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// This project's `*`-wildcard patterns.
    #[default]
    Glob,
    /// nmap's regular expressions.
    Nmap,
}

/// A pattern from a rule.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// `m`/`s` in the glob dialect: anchored or substring, case folded unless
    /// the `=` form asked for exact matching.
    Glob {
        glob: Glob,
        anchored: bool,
        ignore_case: bool,
    },
    /// A regular expression: `r`/`r=` anywhere, and every `m`/`s` in the nmap
    /// dialect. `anchored` pins the match to the first byte.
    Regex { regex: Regex, anchored: bool },
}

impl Pattern {
    /// Parse in the built-in glob dialect. A test helper: production code goes
    /// through [`Pattern::parse_in`], which also carries the flags.
    #[cfg(test)]
    fn parse(text: &str) -> Option<Pattern> {
        Pattern::parse_in(text, Dialect::Glob, "")
    }

    /// Parse a pattern in the given dialect. `flags` are the trailing letters
    /// nmap allows after the pattern (`i` for case-insensitive, `s` for
    /// dot-matches-newline).
    fn parse_in(text: &str, dialect: Dialect, flags: &str) -> Option<Pattern> {
        // `m|pattern|`, `s|pattern|`, `r|pattern|` and the `=` variants, which
        // are the case-sensitive twins.
        let (flag, rest) = text.split_once('|')?;
        let pattern = rest.strip_suffix('|').unwrap_or(rest);
        let (flag, exact) = match flag.strip_suffix('=') {
            Some(flag) => (flag, true),
            None => (flag, false),
        };
        let ignore_case = !exact || flags.contains('i');
        let dotall = flags.contains('s');
        match flag {
            "m" | "s" => {
                let anchored = flag == "m";
                match dialect {
                    Dialect::Glob => Some(Pattern::Glob {
                        glob: Glob::parse(pattern),
                        anchored,
                        ignore_case: !exact,
                    }),
                    Dialect::Nmap => compile_regex(pattern, anchored, ignore_case, dotall),
                }
            }
            // `r`/`r=` are a regular expression in either dialect.
            "r" => compile_regex(pattern, false, ignore_case, dotall),
            _ => None,
        }
    }

    /// Does the response match? Returns the captures, `$1` first.
    pub fn captures(&self, response: &str) -> Option<Vec<String>> {
        match self {
            Pattern::Glob {
                glob,
                anchored,
                ignore_case,
            } => {
                if *anchored {
                    glob.match_anchored(response, *ignore_case)
                } else {
                    glob.match_substring(response, *ignore_case)
                }
            }
            Pattern::Regex { regex, anchored } => {
                let captures = regex.captures(response)?;
                if *anchored && captures.get(0).map(|found| found.start()) != Some(0) {
                    return None;
                }
                // `$1` is the first group, so the full match is dropped.
                Some(
                    (1..captures.len())
                        .map(|index| {
                            captures
                                .get(index)
                                .map(|found| found.as_str().to_string())
                                .unwrap_or_default()
                        })
                        .collect(),
                )
            }
        }
    }
}

/// Compile an nmap-style regex. `anchored` pins it to the first character;
/// `ignore_case` and `dotall` come from the `=` form and the trailing flags.
/// Returns `None` for syntax the Rust engine does not support (look-around,
/// back-references), which the caller counts instead of failing the file.
fn compile_regex(
    pattern: &str,
    anchored: bool,
    ignore_case: bool,
    dotall: bool,
) -> Option<Pattern> {
    // nmap writes `\0` for a NUL byte; Rust's engine wants `\x00`.
    let pattern = pattern.replace("\\0", "\\x00");
    let body = if anchored {
        format!("^(?:{pattern})")
    } else {
        pattern
    };

    let mut flags = String::new();
    if ignore_case {
        flags.push('i');
    }
    if dotall {
        flags.push('s');
    }
    let source = if flags.is_empty() {
        body
    } else {
        format!("(?{flags}){body}")
    };

    Regex::new(&source).ok().map(|regex| Pattern::Regex { regex, anchored })
}

/// One piece of a compiled pattern.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Literal(char),
    /// `?` — exactly one character.
    Any,
    /// `*` — any run of characters, but never across a line break. Captured.
    Star,
    /// `[...]` — one character from a set, with ranges and `!` negation.
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

/// A compiled pattern. `*` captures, so matching walks the pattern and the text
/// together, backtracking when a `*` guessed short.
#[derive(Debug, Clone)]
pub struct Glob {
    tokens: Vec<Token>,
}

impl Glob {
    pub fn parse(pattern: &str) -> Glob {
        let chars: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::with_capacity(chars.len());
        let mut index = 0;
        while index < chars.len() {
            match chars[index] {
                '*' => {
                    // Collapse runs: `**` and `*` mean the same thing.
                    if tokens.last() != Some(&Token::Star) {
                        tokens.push(Token::Star);
                    }
                    index += 1;
                }
                '?' => {
                    tokens.push(Token::Any);
                    index += 1;
                }
                '[' => {
                    let (class, next) = parse_class(&chars, index + 1);
                    match class {
                        Some(class) => {
                            tokens.push(class);
                            index = next;
                        }
                        // An unclosed `[` is just a bracket.
                        None => {
                            tokens.push(Token::Literal('['));
                            index += 1;
                        }
                    }
                }
                other => {
                    tokens.push(Token::Literal(other));
                    index += 1;
                }
            }
        }
        Glob { tokens }
    }

    /// The response must match from its first character, like nmap's `m`. The
    /// pattern does not have to exhaust the response: a service banner usually
    /// has more after the interesting part.
    pub fn match_anchored(&self, text: &str, ignore_case: bool) -> Option<Vec<String>> {
        let chars: Vec<char> = text.chars().collect();
        let mut steps = 0;
        let mut captures = Vec::new();
        if walk(
            &self.tokens,
            &chars,
            0,
            ignore_case,
            &mut captures,
            &mut steps,
        ) {
            return Some(captures);
        }
        None
    }

    /// Any substring of the response may match, like nmap's `s`.
    pub fn match_substring(&self, text: &str, ignore_case: bool) -> Option<Vec<String>> {
        let chars: Vec<char> = text.chars().collect();
        for start in 0..=chars.len() {
            let mut steps = 0;
            let mut captures = Vec::new();
            if walk(
                &self.tokens,
                &chars,
                start,
                ignore_case,
                &mut captures,
                &mut steps,
            ) {
                return Some(captures);
            }
        }
        None
    }
}

/// Try to consume all of `tokens` from `at`, filling `captures` with one entry
/// per `*`. Anchoring is the caller's business: it decides where `at` starts.
fn walk(
    tokens: &[Token],
    text: &[char],
    at: usize,
    ignore_case: bool,
    captures: &mut Vec<String>,
    steps: &mut u32,
) -> bool {
    *steps += 1;
    if *steps > MATCH_STEPS {
        return false;
    }

    let Some((token, rest)) = tokens.split_first() else {
        return true;
    };

    match token {
        Token::Literal(expected) => {
            let Some(found) = text.get(at) else {
                return false;
            };
            if !same_char(*expected, *found, ignore_case) {
                return false;
            }
            walk(rest, text, at + 1, ignore_case, captures, steps)
        }
        Token::Any => {
            text.get(at).is_some() && walk(rest, text, at + 1, ignore_case, captures, steps)
        }
        Token::Class { negated, ranges } => {
            let Some(found) = text.get(at) else {
                return false;
            };
            let inside = ranges
                .iter()
                .any(|(low, high)| found >= low && found <= high)
                || ranges.iter().any(|(low, high)| {
                    // A range like `a-z` should also match `A` in a
                    // case-insensitive pattern.
                    ignore_case && (same_char(*low, *found, true) || same_char(*high, *found, true))
                });
            if inside == *negated {
                return false;
            }
            walk(rest, text, at + 1, ignore_case, captures, steps)
        }
        Token::Star => {
            // Longest first: `Server: nginx*` should capture the whole version,
            // not the empty string that lets an empty tail match.
            let limit = text[at..]
                .iter()
                .position(|ch| *ch == '\n' || *ch == '\r')
                .map_or(text.len(), |offset| at + offset);
            let mut end = limit;
            loop {
                captures.push(text[at..end].iter().collect());
                let before = captures.len();
                if walk(rest, text, end, ignore_case, captures, steps) {
                    return true;
                }
                captures.truncate(before - 1);
                if end == at {
                    return false;
                }
                end -= 1;
            }
        }
    }
}

fn same_char(expected: char, found: char, ignore_case: bool) -> bool {
    if ignore_case {
        expected.eq_ignore_ascii_case(&found)
    } else {
        expected == found
    }
}

/// `[a-z0-9_]` and `[!abc]`, from just after the `[`.
fn parse_class(chars: &[char], from: usize) -> (Option<Token>, usize) {
    let mut index = from;
    let negated = matches!(chars.get(index), Some('!') | Some('^'));
    if negated {
        index += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    while let Some(&current) = chars.get(index) {
        if current == ']' && !first {
            return (Some(Token::Class { negated, ranges }), index + 1);
        }
        first = false;
        match chars.get(index + 1) {
            Some('-') if chars.get(index + 2).is_some_and(|next| *next != ']') => {
                ranges.push((current, chars[index + 2]));
                index += 3;
            }
            _ => {
                ranges.push((current, current));
                index += 1;
            }
        }
    }
    (None, from)
}

// --------------------------------------------------------------- rules

/// One `match` or `softmatch` line.
#[derive(Debug, Clone)]
pub struct Rule {
    pub service: String,
    pub pattern: Pattern,
    /// A softmatch names a fallback service; a later hard match wins over it.
    pub soft: bool,
    pub product: Option<String>,
    pub version: Option<String>,
    pub info: Option<String>,
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub device_type: Option<String>,
    pub cpe: Option<String>,
}

impl Rule {
    /// The match, with `$1`…`$9` filled in from the pattern's captures.
    fn build(&self, captures: &[String], response: &str) -> ServiceMatch {
        let fill = |template: &str| -> Option<String> {
            let mut filled = String::with_capacity(template.len());
            let mut chars = template.chars().peekable();
            while let Some(current) = chars.next() {
                if current != '$' {
                    filled.push(current);
                    continue;
                }
                let mut digits = String::new();
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    digits.push(chars.next().expect("peeked"));
                }
                if digits.is_empty() {
                    filled.push('$');
                    continue;
                }
                // A template that refers to a capture the pattern does not
                // have is not worth guessing at.
                let capture = digits
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| captures.get(n - 1))?;
                filled.push_str(&trim_capture(capture));
            }
            let filled = filled.trim().to_string();
            (!filled.is_empty()).then_some(filled)
        };
        // A version is one token. A template captures whatever followed the
        // product — "1.3.8 Server ready" — so the version is cut at the first
        // space, which is how nmap's own patterns bound it.
        let fill_version = |template: &str| -> Option<String> {
            let filled = fill(template)?;
            let token = filled.split_whitespace().next().unwrap_or_default();
            (!token.is_empty()).then(|| token.trim_matches('/').to_string())
        };

        ServiceMatch {
            service: self.service.clone(),
            product: self.product.as_deref().and_then(fill),
            version: self.version.as_deref().and_then(fill_version),
            info: self.info.as_deref().and_then(fill),
            hostname: self.hostname.as_deref().and_then(fill),
            os: self.os.as_deref().and_then(fill),
            device_type: self.device_type.as_deref().and_then(fill),
            cpe: self.cpe.as_deref().and_then(fill),
            response: response.trim().chars().take(200).collect(),
        }
    }
}

/// A captured run, tidied: no trailing control characters, no leading space.
fn trim_capture(capture: &str) -> String {
    capture
        .trim_matches(|ch: char| ch.is_control() && ch != ' ' || ch == '\t')
        .trim()
        .to_string()
}

// --------------------------------------------------------------- probes

/// A probe: bytes to send, ports it applies to, and the rules for its answer.
#[derive(Debug, Clone)]
pub struct Probe {
    pub name: String,
    pub data: Vec<u8>,
    /// Ports this probe is tried on first. Empty means "any port".
    pub ports: Vec<u16>,
    /// Ports this probe is meant for over TLS. Parsed so an SSL-only probe is
    /// not mistaken for a plaintext one; TLS is handled outside this engine.
    pub ssl_ports: Vec<u16>,
    pub rarity: u8,
    /// Wait time this probe asked for, in milliseconds.
    pub total_wait_ms: Option<u32>,
    /// Probes named by a `fallback` directive, in order.
    pub fallback_names: Vec<String>,
    /// The resolved fallback chain: this probe, then the named ones, then
    /// `NULL`. Filled in by [`Engine::resolve_fallbacks`].
    fallback_chain: Vec<usize>,
    pub rules: Vec<Rule>,
}

impl Probe {
    /// A probe that names no port is offered on every plaintext port. A probe
    /// that names only `sslports` is not: its bytes belong inside a TLS
    /// session, so it waits for TLS support rather than guessing.
    fn is_plaintext_generic(&self) -> bool {
        self.ports.is_empty() && self.ssl_ports.is_empty()
    }

    /// Probes are ranked by rarity, low first. An absent rarity (0) sorts
    /// last, because nmap treats 0 as "unset", not as "most common".
    fn rarity_rank(&self) -> u8 {
        if self.rarity == 0 { u8::MAX } else { self.rarity }
    }

    /// Send the probe and read the answer, as raw bytes: a banner is not always
    /// text, and a binary service probe needs the bytes to match against.
    /// `None` when the port is not actually listening any more, or answers
    /// nothing.
    pub fn run(&self, ip: Ipv4Addr, port: u16, timeout: Duration) -> Option<Vec<u8>> {
        let address = SocketAddr::from((ip, port));
        let wait = self
            .total_wait_ms
            .map(|ms| Duration::from_millis(u64::from(ms)))
            .unwrap_or(timeout)
            .min(timeout.max(Duration::from_millis(50)));
        let mut stream = TcpStream::connect_timeout(&address, timeout).ok()?;
        stream.set_read_timeout(Some(wait)).ok();
        stream.set_write_timeout(Some(wait)).ok();

        if !self.data.is_empty() {
            stream.write_all(&self.data).ok()?;
            stream.flush().ok();
        }

        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        while buffer.len() < RESPONSE_LIMIT {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                // A timeout after some bytes is a complete answer for our
                // purposes; a timeout with nothing is no answer at all.
                Err(_) => break,
            }
        }
        if buffer.is_empty() {
            return None;
        }
        Some(buffer)
    }

    /// The first hard match, or the first softmatch when nothing hard matched.
    pub fn identify(&self, response: &str) -> Option<ServiceMatch> {
        let (hard, soft) = Probe::first_match(&self.rules, response, response);
        hard.or(soft)
    }

    /// Fold a raw response into text: Latin-1, so every byte maps to one
    /// character and a `\x00` probe can still match; nmap's own patterns are
    /// written in those terms.
    fn latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|&byte| byte as char).collect()
    }

    /// The first hard rule that matches, and the first softmatch if none does.
    /// `text` is what patterns run against; `display` is what the result keeps.
    fn first_match(rules: &[Rule], text: &str, display: &str) -> (Option<ServiceMatch>, Option<ServiceMatch>) {
        let mut soft = None;
        for rule in rules {
            let Some(captures) = rule.pattern.captures(text) else {
                continue;
            };
            if rule.soft {
                soft.get_or_insert_with(|| rule.build(&captures, display));
                continue;
            }
            return (Some(rule.build(&captures, display)), None);
        }
        (None, soft)
    }
}

// --------------------------------------------------------------- engine

/// The probe table, and what can be said with it.
#[derive(Debug, Default)]
pub struct Engine {
    probes: Vec<Probe>,
    /// Ports an `Exclude` line told us to leave alone.
    excluded: HashSet<u16>,
    /// The `NULL` probe, whose rules read a banner no other probe could place.
    /// `None` when the table has none.
    null_index: Option<usize>,
    /// `r`/`r=` rules that were skipped, reported once by the caller. A rule
    /// lands here when it is a regular expression this engine cannot compile
    /// (look-around, back-references), not because it is a regex at all.
    pub skipped_regex: usize,
}

impl Engine {
    /// Parse the `nmap-service-probes` format in this project's glob dialect,
    /// which is what the built-in table is written in.
    ///
    /// Unknown directives, unknown match fields and malformed lines are skipped:
    /// a probe file is data, and a foreign one should degrade rather than abort
    /// a scan.
    pub fn parse(text: &str) -> Engine {
        Engine::parse_with(text, Dialect::Glob)
    }

    /// Parse a probe table in a chosen dialect. [`Engine::load`] uses
    /// [`Dialect::Nmap`] for a supplied file, because that is what nmap's own
    /// `nmap-service-probes` is written in.
    pub fn parse_with(text: &str, dialect: Dialect) -> Engine {
        let mut engine = Engine::default();
        // nmap allows a probe's rules to be split across lines, so rules are
        // attached to the probe that most recently opened.
        let mut current: Option<Probe> = None;

        for (number, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (directive, rest) = split_directive(line);
            match directive {
                "Exclude" => {
                    engine.excluded.extend(parse_exclude(rest));
                }
                "Probe" => {
                    if let Some(probe) = current.take() {
                        engine.probes.push(probe);
                    }
                    current = parse_probe(rest);
                    if current.is_none() {
                        eprintln!("probe file line {}: unusable probe, skipped", number + 1);
                    }
                }
                "match" | "softmatch" => {
                    let Some(probe) = current.as_mut() else {
                        continue;
                    };
                    match parse_rule(directive == "softmatch", rest, dialect) {
                        Ok(Some(rule)) => probe.rules.push(rule),
                        Ok(None) => engine.skipped_regex += 1,
                        Err(reason) => {
                            eprintln!("probe file line {}: {reason}", number + 1);
                        }
                    }
                }
                // A directive that belongs to the probe above it.
                "ports" => {
                    if let Some(probe) = current.as_mut() {
                        probe.ports = parse_port_list(rest);
                    }
                }
                "sslports" => {
                    if let Some(probe) = current.as_mut() {
                        probe.ssl_ports = parse_port_list(rest);
                    }
                }
                "rarity" => {
                    if let Some(probe) = current.as_mut() {
                        probe.rarity = rest.trim().parse().unwrap_or(0);
                    }
                }
                "totalwaitms" => {
                    if let Some(probe) = current.as_mut() {
                        probe.total_wait_ms = rest.trim().parse().ok();
                    }
                }
                "fallback" => {
                    if let Some(probe) = current.as_mut() {
                        probe.fallback_names = rest
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                            .collect();
                    }
                }
                // `tcpwrappedms` and anything newer: no effect here.
                _ => {}
            }
        }
        if let Some(probe) = current {
            engine.probes.push(probe);
        }
        engine.resolve_fallbacks();
        engine
    }

    /// The built-in table, parsed from the same format a file would use — one
    /// dialect, one code path, no divergence between the two.
    pub fn embedded() -> Engine {
        Engine::parse(EMBEDDED_PROBES)
    }

    /// The built-in table plus probes from a file, if one was configured. A
    /// supplied file's probes are tried first, because it is the more specific
    /// table.
    pub fn load(path: Option<&std::path::Path>) -> Engine {
        let Some(path) = path else {
            return Engine::embedded();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => {
                // A supplied file is nmap's own format, not this project's glob
                // dialect, so it is parsed as nmap regular expressions.
                let mut engine = Engine::parse_with(&text, Dialect::Nmap);
                let from_file = engine.probes.len();
                let extra = Engine::embedded();
                engine.skipped_regex += extra.skipped_regex;
                engine.probes.extend(extra.probes);
                if from_file == 0 {
                    eprintln!(
                        "probe file {}: no usable probes, falling back to the built-in table",
                        path.display()
                    );
                }
                // The merged table has new indices, so the chains are rebuilt
                // against it: a file probe may fall back to a built-in one.
                engine.resolve_fallbacks();
                engine
            }
            Err(e) => {
                eprintln!(
                    "probe file {}: {e}, falling back to the built-in table",
                    path.display()
                );
                Engine::embedded()
            }
        }
    }

    pub fn len(&self) -> usize {
        self.probes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }

    /// Resolve every probe's `fallback` names into a chain of indices, the way
    /// nmap's `AllProbes::compileFallbacks` does: the probe itself, then the
    /// probes it names, then `NULL`. A response the sending probe cannot place
    /// is offered to each rule set in turn, which is what lets a banner read by
    /// one probe still be named by another probe's rules.
    fn resolve_fallbacks(&mut self) {
        let by_name: std::collections::HashMap<String, usize> = self
            .probes
            .iter()
            .enumerate()
            .map(|(index, probe)| (probe.name.clone(), index))
            .collect();
        self.null_index = by_name.get("NULL").copied();

        for index in 0..self.probes.len() {
            let mut chain = vec![index];
            for name in &self.probes[index].fallback_names {
                if let Some(&fallback) = by_name.get(name.as_str())
                    && !chain.contains(&fallback)
                {
                    chain.push(fallback);
                }
            }
            // Every probe falls back to `NULL` last, so a passive banner that
            // arrives in answer to any probe is still read. The `NULL` probe
            // itself has nothing to fall back to.
            if let Some(null) = self.null_index
                && !chain.contains(&null)
            {
                chain.push(null);
            }
            self.probes[index].fallback_chain = chain;
        }
    }

    /// The probes to try on a port, as indices: the ones that name that port
    /// first, then the generic ones. Within each group the lowest rarity comes
    /// first, and an absent rarity sorts last. `Exclude`d ports get nothing.
    fn candidate_indices(&self, port: u16) -> Vec<usize> {
        if self.excluded.contains(&port) {
            return Vec::new();
        }
        let mut specific: Vec<usize> = (0..self.probes.len())
            .filter(|&index| self.probes[index].ports.contains(&port))
            .collect();
        let mut generic: Vec<usize> = (0..self.probes.len())
            .filter(|&index| self.probes[index].is_plaintext_generic())
            .collect();
        // Stable sort: probes of equal rarity keep the table's order, which is
        // how nmap's own file lists most-common first.
        specific.sort_by_key(|&index| self.probes[index].rarity_rank());
        generic.sort_by_key(|&index| self.probes[index].rarity_rank());
        specific
            .into_iter()
            .chain(generic)
            .take(PROBES_PER_PORT)
            .collect()
    }

    /// The probes to try on a port: the ones that name that port first (lowest
    /// rarity first), then the generic ones. `Exclude`d ports get nothing.
    pub fn probes_for(&self, port: u16) -> Vec<&Probe> {
        self.candidate_indices(port)
            .into_iter()
            .map(|index| &self.probes[index])
            .collect()
    }

    /// Identify what is listening on one port.
    pub fn identify_port(
        &self,
        ip: Ipv4Addr,
        port: u16,
        timeout: Duration,
    ) -> Option<ServiceMatch> {
        for index in self.candidate_indices(port) {
            let Some(response) = self.probes[index].run(ip, port, timeout) else {
                continue;
            };
            if let Some(found) = self.match_response(index, &response) {
                return Some(found);
            }
        }
        None
    }

    /// Place a response using a probe's rules and, when none of them match, the
    /// rules of its fallback chain. A softmatch is kept as a last resort, the
    /// same bargain nmap's `servicescan_read_handler` makes.
    fn match_response(&self, index: usize, response: &[u8]) -> Option<ServiceMatch> {
        let text = Probe::latin1(response);
        let display = String::from_utf8_lossy(response);
        let mut soft = None;
        for &fallback in &self.probes[index].fallback_chain {
            let Some(probe) = self.probes.get(fallback) else {
                continue;
            };
            let (hard, fallback_soft) = Probe::first_match(&probe.rules, &text, &display);
            if let Some(hard) = hard {
                return Some(hard);
            }
            if soft.is_none() {
                soft = fallback_soft;
            }
        }
        soft
    }

    /// Identify what is listening on several ports, in order.
    pub fn identify(
        &self,
        ip: Ipv4Addr,
        ports: &[u16],
        timeout: Duration,
    ) -> Vec<(u16, ServiceMatch)> {
        ports
            .iter()
            .filter_map(|port| {
                self.identify_port(ip, *port, timeout)
                    .map(|found| (*port, found))
            })
            .collect()
    }
}

/// Split `Probe TCP NULL q||` into `("Probe", "TCP NULL q||")`.
fn split_directive(line: &str) -> (&str, &str) {
    match line.split_once(char::is_whitespace) {
        Some((directive, rest)) => (directive, rest.trim()),
        None => (line, ""),
    }
}

/// `Exclude T:9100,9200 U:53` → the TCP ports, which is all this engine serves.
fn parse_exclude(rest: &str) -> Vec<u16> {
    rest.split_whitespace()
        .filter_map(|part| part.strip_prefix("T:"))
        .flat_map(parse_port_list)
        .collect()
}

/// `80,443,8000-8010`.
fn parse_port_list(rest: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for part in rest.trim().split(',') {
        let part = part.trim();
        match part.split_once('-') {
            Some((from, to)) => {
                let (Ok(from), Ok(to)) = (from.trim().parse::<u16>(), to.trim().parse::<u16>())
                else {
                    continue;
                };
                ports.extend(from..=to);
            }
            None => {
                if let Ok(port) = part.parse::<u16>() {
                    ports.push(port);
                }
            }
        }
    }
    ports
}

/// `TCP NULL q|GET / HTTP/1.0\r\n\r\n|`.
fn parse_probe(rest: &str) -> Option<Probe> {
    let mut parts = rest.split_whitespace();
    let protocol = parts.next()?;
    // UDP probes are not implemented; skip them rather than pretend.
    if !protocol.eq_ignore_ascii_case("TCP") {
        return None;
    }
    let name = parts.next()?.to_string();
    let body = parts.collect::<Vec<_>>().join(" ");
    let data = match body.split_once('|') {
        Some((_, rest)) => decode_escapes(rest.rsplit_once('|').map_or(rest, |(data, _)| data)),
        None => Vec::new(),
    };
    Some(Probe {
        name,
        data,
        ports: Vec::new(),
        ssl_ports: Vec::new(),
        rarity: 0,
        total_wait_ms: None,
        fallback_names: Vec::new(),
        fallback_chain: Vec::new(),
        rules: Vec::new(),
    })
}

/// `ssh m|SSH-2.0-OpenSSH_*| p/OpenSSH/ v/$1/`.
fn parse_rule(
    soft: bool,
    rest: &str,
    dialect: Dialect,
) -> std::result::Result<Option<Rule>, String> {
    let (service, rest) = rest
        .split_once(char::is_whitespace)
        .ok_or_else(|| "a match line with no pattern".to_string())?;

    let (kind, rest) = rest
        .split_once('|')
        .ok_or_else(|| "a match line whose pattern is not delimited by `|`".to_string())?;
    let (pattern_text, fields) = rest
        .split_once('|')
        .ok_or_else(|| format!("`{kind}` pattern `{rest}` is not closed"))?;

    // nmap allows flags (`i`, `s`) between the pattern and the version fields.
    let (flags, fields) = split_flags(fields);

    let Some(pattern) = Pattern::parse_in(&format!("{kind}|{pattern_text}"), dialect, flags) else {
        // A rule this engine cannot compile: counted as skipped, not a failure.
        return Ok(None);
    };

    let mut rule = Rule {
        service: service.to_string(),
        pattern,
        soft,
        product: None,
        version: None,
        info: None,
        hostname: None,
        os: None,
        device_type: None,
        cpe: None,
    };
    for field in split_fields(fields) {
        let (key, value) = match field.split_once('/') {
            Some(split) => split,
            None => continue,
        };
        // `p/OpenSSH/` — the trailing slash is the delimiter, not the value.
        let value = value.strip_suffix('/').unwrap_or(value);
        match key {
            "p" => rule.product = Some(value.to_string()),
            "v" => rule.version = Some(value.to_string()),
            "i" => rule.info = Some(value.to_string()),
            "h" => rule.hostname = Some(value.to_string()),
            "o" => rule.os = Some(value.to_string()),
            "d" => rule.device_type = Some(value.to_string()),
            "cpe" => rule.cpe = Some(format!("cpe:{value}")),
            // `a` (nmap's newer alias) and anything unknown: ignored.
            _ => {}
        }
    }
    Ok(Some(rule))
}

/// Split the flags nmap puts after a match pattern from the version fields.
/// A field always contains a `/`; a flags token is a run of lower-case letters.
fn split_flags(fields: &str) -> (&str, &str) {
    let trimmed = fields.trim_start();
    let (token, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((token, rest)) => (token, rest),
        None => (trimmed, ""),
    };
    let is_flags = !token.is_empty()
        && !token.contains('/')
        && token.chars().all(|ch| ch.is_ascii_lowercase());
    if is_flags {
        (token, rest.trim_start())
    } else {
        ("", fields)
    }
}

/// Split `p/OpenSSH/ v/$1/ cpe:/a:openbsd:openssh/` on whitespace, keeping
/// `/`-delimited values that contain spaces together.
fn split_fields(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut open = false;
    for ch in text.chars() {
        match ch {
            '/' => {
                open = !open;
                current.push(ch);
            }
            ch if ch.is_whitespace() && !open => {
                if !current.is_empty() {
                    fields.push(std::mem::take(&mut current));
                }
            }
            ch => current.push(ch),
        }
    }
    if !current.is_empty() {
        fields.push(current);
    }
    fields
}

/// nmap's probe escapes: `\r`, `\n`, `\t`, `\0`, `\xHH` and `\\`.
fn decode_escapes(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(current) = chars.next() {
        if current != '\\' {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(current.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('r') => bytes.push(b'\r'),
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('0') => bytes.push(0),
            Some('a') => bytes.push(7),
            Some('\\') => bytes.push(b'\\'),
            Some('x') | Some('X') => {
                let mut hex = String::new();
                while hex.len() < 2 && chars.peek().is_some_and(|ch| ch.is_ascii_hexdigit()) {
                    hex.push(chars.next().expect("peeked"));
                }
                match u8::from_str_radix(&hex, 16) {
                    Ok(byte) => bytes.push(byte),
                    Err(_) => {
                        bytes.push(b'x');
                        bytes.extend_from_slice(hex.as_bytes());
                    }
                }
            }
            Some(other) => {
                bytes.push(b'\\');
                let mut buffer = [0u8; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    bytes
}

// ------------------------------------------------------- the built-in table

/// Probes written for this project: the services a LAN actually runs, and the
/// words that give them away. Order matters — the first rule that matches wins,
/// so specific products come before their generic service.
const EMBEDDED_PROBES: &str = r#"
# ------------------------------------------------ who speaks first
Probe TCP NULL q||
# SSH: the banner names the implementation.
match ssh m|SSH-2.0-OpenSSH_*| p/OpenSSH/ v/$1/ cpe:/a:openbsd:openssh/
match ssh m|SSH-1.99-OpenSSH_*| p/OpenSSH/ v/$1/ cpe:/a:openbsd:openssh/
match ssh m|SSH-2.0-dropbear_*| p/Dropbear SSH/ v/$1/
match ssh m|SSH-1.99-dropbear_*| p/Dropbear SSH/ v/$1/
match ssh m|SSH-2.0-Cisco-*| p/Cisco SSH/ v/$1/ d/router/
match ssh m|SSH-1.99-Cisco-*| p/Cisco SSH/ v/$1/ d/router/
match ssh m|SSH-2.0-ROSSSH*| p/RouterOS SSH/ v/$1/ d/router/
match ssh m|SSH-2.0-*| p/$1/
match ssh m|SSH-*| p/$1/
# Mail, before FTP: both open with `220`.
match smtp m|220*Postfix*| p/Postfix/
match smtp m|220*Exim *| p/Exim/ v/$2/
match smtp m|220*Sendmail *| p/Sendmail/ v/$2/
match smtp m|220*Microsoft ESMTP*| p/Microsoft Exchange/
match smtp m|220*ESMTP*|
match smtp m|220*SMTP*|
match pop3 m|+OK*Dovecot*| p/Dovecot/
match pop3 m|+OK*|
match imap m|* OK*Dovecot*| p/Dovecot/
match imap m|* OK*|
match ftp m|220*ProFTPD *| p/ProFTPD/ v/$2/
match ftp m|220*vsFTPd *| p/vsFTPd/ v/$2/
match ftp m|220*FileZilla Server *| p/FileZilla Server/ v/$2/
match ftp m|220*Pure-FTPd*| p/Pure-FTPd/
match ftp m|220*|
match mariadb m|*MariaDB*| p/MariaDB/
match mysql m|*mysql_native_password*| p/MySQL/ o/Linux/
match mysql m|*caching_sha2_password*| p/MySQL/ o/Linux/
match vnc m|RFB *| p/VNC/ v/$1/
match redis m|-NOAUTH*| p/Redis/
match redis m|-ERR*| p/Redis/
match telnet m|\xff\xfd*| p/Telnet/
match rtsp m|RTSP/1.0 *| p/RTSP/ i/$1/

# ------------------------------------------------ after a nudge
Probe TCP GenericLines q|\r\n\r\n|
match http m|HTTP/1.[01] *| p/HTTP/ i/$1/
match sip m|SIP/2.0 *| p/SIP/

Probe TCP GetRequest q|GET / HTTP/1.0\r\n\r\n|
match http s|Server: nginx/*| p/nginx/ v/$1/
match http s|Server: openresty/*| p/OpenResty/ v/$1/
match http s|Server: Apache/*| p/Apache httpd/ v/$1/
match http s|Server: Apache-Coyote/*| p/Apache Tomcat/ v/$1/
match http s|Server: Microsoft-IIS/*| p/Microsoft IIS/ v/$1/ o/Windows/
match http s|Server: Microsoft-HTTPAPI/*| p/Microsoft HTTPAPI/ v/$1/ o/Windows/
match http s|Server: Kestrel| p/Kestrel/ o/Windows/
match http s|Server: lighttpd/*| p/lighttpd/ v/$1/
match http s|Server: thttpd/*| p/thttpd/ v/$1/
match http s|Server: Boa/*| p/Boa/ v/$1/
match http s|Server: MiniServ/*| p/Webmin/ v/$1/
match http s|Server: micro_httpd*| p/micro_httpd/
match http s|Server: RomPager/*| p/RomPager/ v/$1/ d/router/
match http s|Server: GoAhead-Webs/*| p/GoAhead WebServer/ v/$1/
match http s|Server: GoAhead/*| p/GoAhead/ v/$1/
match http s|Server: cloudflare| p/Cloudflare/
match http s|Server: Werkzeug/*| p/Werkzeug/ v/$1/
match http s|Server: gunicorn/*| p/gunicorn/ v/$1/
match http s|Server: uvicorn*| p/uvicorn/
match http s|Server: envoy| p/Envoy proxy/
match http s|Server: Caddy| p/Caddy/
match http s|Server: LiteSpeed| p/LiteSpeed/
match http s|Server: Jetty(*)| p/Jetty/ v/$1/
match http s|Server: Cowboy| p/Cowboy/
match http s|Server: BaseHTTP/*| p/BaseHTTP/ v/$1/
match http s|X-Powered-By: Express| p/Express/
match http s|X-Powered-By: ASP.NET| p/ASP.NET/ o/Windows/
match http s|X-Powered-By: PHP/*| p/PHP/ v/$1/
match http s|X-Jenkins: *| p/Jenkins/ v/$1/
match http s|Server: *| p/$1/
# The generic line goes last: a product rule above must win over it.
match http m|HTTP/1.[01] *| p/HTTP/ i/$1/

Probe TCP HTTPOptions q|OPTIONS / HTTP/1.0\r\n\r\n|
match http m|HTTP/1.[01] *| p/HTTP/ i/$1/

# ------------------------------------------------ one service per port
Probe TCP RTSPRequest q|OPTIONS / RTSP/1.0\r\n\r\n|
ports 554,8554
match rtsp m|RTSP/1.0 *| p/RTSP/ i/$1/
match rtsp s|Server: *| p/$1/

Probe TCP RedisInfo q|INFO\r\n|
ports 6379
match redis s|redis_version:*| p/Redis/ v/$1/
match redis m|-NOAUTH*| p/Redis/
match redis m|-ERR*| p/Redis/

Probe TCP MemcachedStats q|stats\r\n|
ports 11211
match memcached s|STAT version *| p/memcached/ v/$1/

Probe TCP MqttConnect q|\x10\x0c\x00\x04MQTT\x04\x02\x00\x3c\x00\x00|
ports 1883,8883
match mqtt m|\x20\x02*| p/MQTT/
match mqtt m|*MQTT*| p/MQTT/

Probe TCP PostgreSQL q|\x00\x00\x00\x08\x04\xd2\x16\x2f|
ports 5432
match postgresql m|S| p/PostgreSQL/
match postgresql m|N| p/PostgreSQL/
match postgresql m|E*| p/PostgreSQL/

Probe TCP PjlInfo q|\x1b%-12345X@PJL INFO ID\r\n|
ports 9100,515,631
match jetdirect s|@PJL INFO ID*| p/HP JetDirect/ i/$1/ d/printer/
match jetdirect m|*@PJL*| p/HP JetDirect/ d/printer/

Probe TCP SipOptions q|OPTIONS sip:nm SIP/2.0\r\nVia: SIP/2.0/TCP nm;branch=foo\r\nFrom: <sip:nm@nm>;tag=root\r\nTo: <sip:nm2@nm2>\r\nCall-ID: 50000\r\nCSeq: 42 OPTIONS\r\nMax-Forwards: 70\r\nContent-Length: 0\r\nContact: <sip:nm@nm>\r\nAccept: application/sdp\r\n\r\n|
ports 5060,5061
match sip m|SIP/2.0 *| p/SIP/ i/$1/

# ------------------------------------------------ HTTP APIs
Probe TCP ElasticsearchInfo q|GET / HTTP/1.0\r\n\r\n|
ports 9200
match elasticsearch s|"tagline"*| p/Elasticsearch/
match elasticsearch s|"cluster_name"*| p/Elasticsearch/

Probe TCP DockerVersion q|GET /version HTTP/1.0\r\n\r\n|
ports 2375,2376
match docker s|"ApiVersion"*| p/Docker Engine/
match docker s|"MinAPIVersion"*| p/Docker Engine/

Probe TCP KubernetesVersion q|GET /version HTTP/1.0\r\n\r\n|
ports 6443,10250
match kubernetes s|"gitVersion"*| p/Kubernetes/

Probe TCP MongoIsMaster q|\x39\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xd4\x07\x00\x00\x00\x00\x00\x00admin.$cmd\x00\x00\x00\x00\x00\xff\xff\xff\xff\x12\x00\x00\x00\x10ismaster\x00\x01\x00\x00\x00|
ports 27017,27018
match mongodb s|ismaster*| p/MongoDB/
match mongodb s|maxWireVersion*| p/MongoDB/
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::embedded()
    }

    #[test]
    fn the_built_in_table_loads_and_covers_the_common_services() {
        let engine = engine();
        assert!(engine.len() >= 10, "{} probes", engine.len());
        // Every probe has rules, and every rule has a service name.
        for probe in &engine.probes {
            assert!(!probe.rules.is_empty(), "{} has no rules", probe.name);
            for rule in &probe.rules {
                assert!(!rule.service.is_empty());
            }
        }
    }

    #[test]
    fn a_server_header_names_the_product_before_the_generic_http_rule() {
        let engine = engine();
        let get = engine
            .probes
            .iter()
            .find(|probe| probe.name == "GetRequest")
            .expect("the built-in HTTP probe");

        let found = get
            .identify("HTTP/1.0 200 OK\r\nServer: nginx/1.24.0\r\n\r\n")
            .expect("a server header should be named");
        assert_eq!(found.product.as_deref(), Some("nginx"));
        assert_eq!(found.version.as_deref(), Some("1.24.0"));

        // With no product header the generic HTTP rule still names the service.
        let found = get
            .identify("HTTP/1.0 204 No Content\r\n\r\n")
            .expect("the generic rule");
        assert_eq!(found.product.as_deref(), Some("HTTP"));
    }

    #[test]
    fn probes_are_picked_by_port_then_by_generality() {
        let engine = engine();
        let on_redis = engine.probes_for(6379);
        assert_eq!(on_redis[0].name, "RedisInfo", "the port's own probe first");
        assert!(
            on_redis.iter().any(|probe| probe.name == "NULL"),
            "then the generic ones"
        );
        // And never more than the cap, however many apply.
        for port in [22, 80, 443, 5432, 9100, 31337] {
            assert!(engine.probes_for(port).len() <= PROBES_PER_PORT);
        }
    }

    #[test]
    fn the_pattern_dialect_matches_nmaps_literal_and_substring_forms() {
        // `m` is anchored at the start and case-insensitive.
        let pattern = Pattern::parse("m|SSH-2.0-OpenSSH_*|").expect("pattern");
        let captures = pattern
            .captures("ssh-2.0-OpenSSH_9.6p1 Ubuntu\n")
            .expect("match");
        assert_eq!(
            captures[0], "9.6p1 Ubuntu",
            "case-insensitive, and `*` stops at the newline"
        );
        assert!(pattern.captures("220 FTP ready").is_none(), "anchored");

        // `s` is not anchored.
        let pattern = Pattern::parse("s|Server: nginx*|").expect("pattern");
        let response = "HTTP/1.0 200 OK\r\nServer: nginx/1.24.0 (Ubuntu)\r\nX: y\r\n\r\n";
        assert_eq!(
            pattern.captures(response).expect("match")[0],
            "/1.24.0 (Ubuntu)"
        );

        // `?` and a character class.
        let pattern = Pattern::parse("m|HTTP/1.[01] *|").expect("pattern");
        assert!(pattern.captures("HTTP/1.1 200 OK").is_some());
        assert!(pattern.captures("HTTP/1.0 200 OK").is_some());
        assert!(pattern.captures("HTTP/2 200 OK").is_none());
        let digits = Pattern::parse("m|[0-9][0-9][0-9] *|").expect("pattern");
        assert!(digits.captures("220 ready").is_some());
        assert!(digits.captures("22 ready").is_none());
    }

    #[test]
    fn a_regex_rule_is_compiled_and_matches() {
        let engine = Engine::parse(
            "Probe TCP R q|hello|\nmatch thing r|^h.*o$| p/Regexy/\nmatch other m|hello| p/Gloppy/\n",
        );
        assert_eq!(engine.skipped_regex, 0);
        assert_eq!(engine.probes[0].rules.len(), 2);
        // The regex rule comes first, so it wins over the glob rule that also
        // matches the same greeting.
        let found = engine.probes[0].identify("hello").expect("match");
        assert_eq!(found.service, "thing");
        assert_eq!(found.product.as_deref(), Some("Regexy"));
    }

    #[test]
    fn a_rule_the_regex_engine_cannot_compile_is_counted() {
        // Rust's regex engine has no look-around; that rule is skipped and the
        // next one still loads.
        let engine = Engine::parse(
            "Probe TCP R q|x|\nmatch x r|(?=foo)bar| p/Thing/\nmatch y r|x| p/Other/\n",
        );
        assert_eq!(engine.skipped_regex, 1, "reported, not silently dropped");
        assert_eq!(engine.probes[0].rules.len(), 1, "the usable rule stays");
    }

    #[test]
    fn the_nmap_dialect_reads_a_regular_expression_match() {
        let engine = Engine::parse_with(
            "Probe TCP NULL q||\n\
             match ssh m|^SSH-2\\.0-OpenSSH_([\\w.]+)| p/OpenSSH/ v/$1/\n",
            Dialect::Nmap,
        );
        let found = engine.probes[0]
            .identify("SSH-2.0-OpenSSH_9.6p1 Ubuntu\r\n")
            .expect("match");
        assert_eq!(found.product.as_deref(), Some("OpenSSH"));
        assert_eq!(found.version.as_deref(), Some("9.6p1"));

        // `m` is anchored: the pattern appearing later on the line is not a hit.
        assert!(
            engine.probes[0]
                .identify("prefix SSH-2.0-OpenSSH_9.6p1")
                .is_none(),
            "`m` must match from the first character"
        );
    }

    #[test]
    fn nmap_binary_escapes_match_the_bytes_they_name() {
        // `\0` is nmap's NUL; `\x01` its hex escape. Both must survive the
        // byte-to-Latin-1 conversion the matcher runs on.
        let engine = Engine::parse_with(
            "Probe TCP P q|x|\nmatch thing m|^\\0\\0\\x01ABC| p/Binary/\n",
            Dialect::Nmap,
        );
        let found = engine.match_response(0, b"\x00\x00\x01ABC rest");
        assert_eq!(found.expect("match").product.as_deref(), Some("Binary"));
    }

    #[test]
    fn version_templates_fill_in_from_the_captures() {
        let engine = Engine::parse(
            "Probe TCP P q|x|\nmatch ssh m|SSH-2.0-OpenSSH_*| p/OpenSSH/ v/$1/ i/ref $1/\n",
        );
        let found = engine.probes[0]
            .identify("SSH-2.0-OpenSSH_9.6p1\r\n")
            .expect("match");
        assert_eq!(found.service, "ssh");
        assert_eq!(found.product.as_deref(), Some("OpenSSH"));
        assert_eq!(found.version.as_deref(), Some("9.6p1"));
        assert_eq!(
            found.info.as_deref(),
            Some("ref 9.6p1"),
            "the capture is reusable"
        );
        assert_eq!(found.describe().as_deref(), Some("OpenSSH 9.6p1"));
    }

    #[test]
    fn a_softmatch_yields_to_a_later_hard_match() {
        let engine = Engine::parse(
            "Probe TCP P q|x|\n\
             softmatch http s|Server: *| p/$1/\n\
             match http m|HTTP/1.1 *| p/HTTP/\n",
        );
        let found = engine.probes[0]
            .identify("HTTP/1.1 200 OK\r\nServer: x\r\n")
            .expect("match");
        assert_eq!(
            found.product.as_deref(),
            Some("HTTP"),
            "the hard match wins"
        );

        // With no hard match, the softmatch is used rather than nothing.
        let probe = &engine.probes[0];
        let found = probe
            .identify("Server: GoAhead-Webs\r\n")
            .expect("softmatch");
        assert_eq!(found.product.as_deref(), Some("GoAhead-Webs"));
    }

    #[test]
    fn probe_escapes_are_decoded() {
        assert_eq!(
            decode_escapes(r"GET / HTTP/1.0\r\n\r\n"),
            b"GET / HTTP/1.0\r\n\r\n"
        );
        assert_eq!(decode_escapes(r"\x1b%-12345X"), b"\x1b%-12345X");
        assert_eq!(decode_escapes(r"\x00\x0a"), vec![0, 10]);
        assert_eq!(decode_escapes(r"a\\b"), b"a\\b");
        assert_eq!(decode_escapes(r"\t"), b"\t");
    }

    #[test]
    fn the_file_format_parses_directives_ports_and_comments() {
        let engine = Engine::parse(
            "# a comment\n\
             Exclude T:9100,9200\n\
             Probe TCP Thing q|hi| \n\
             ports 80,8000-8002\n\
             rarity 5\n\
             totalwaitms 1200\n\
             match thing m|hi*| p/Thing/ v/$1/\n",
        );
        assert_eq!(engine.len(), 1);
        let probe = &engine.probes[0];
        assert_eq!(probe.data, b"hi");
        assert_eq!(probe.ports, vec![80, 8000, 8001, 8002]);
        assert_eq!(probe.rarity, 5);
        assert_eq!(probe.total_wait_ms, Some(1200));
        assert!(engine.probes_for(9100).is_empty(), "excluded");
        assert!(
            !engine
                .probes_for(9200)
                .iter()
                .any(|probe| probe.name == "Thing")
        );
    }

    #[test]
    fn a_fallback_probe_reads_a_banner_the_sending_probe_cannot_place() {
        // A response to GetRequest goes through GetRequest's own rules first,
        // then the rules of the probes it falls back to — nmap's chain.
        let engine = Engine::parse(
            "Probe TCP NULL q||\nmatch ftp m|220 *| p/from NULL/\n\
             Probe TCP GetRequest q|GET / HTTP/1.0\r\n\r\n|\n\
             fallback NULL\n\
             match http m|HTTP/1.[01] *| p/from GetRequest/\n",
        );
        let get = engine
            .probes
            .iter()
            .position(|probe| probe.name == "GetRequest")
            .expect("probe");

        // Its own rule wins when it matches.
        let found = engine
            .match_response(get, b"HTTP/1.1 200 OK\r\n")
            .expect("own rule");
        assert_eq!(found.product.as_deref(), Some("from GetRequest"));

        // Otherwise the fallback's rule names the greeting.
        let found = engine
            .match_response(get, b"220 ProFTPD ready\r\n")
            .expect("the fallback's rule matches");
        assert_eq!(found.product.as_deref(), Some("from NULL"));
    }

    #[test]
    fn every_probe_falls_back_to_null_even_without_a_directive() {
        let engine = Engine::parse(
            "Probe TCP NULL q||\nmatch ftp m|220 *| p/from NULL/\n\
             Probe TCP Thing q|x|\nmatch thing m|x| p/Thing/\n",
        );
        let thing = engine
            .probes
            .iter()
            .position(|probe| probe.name == "Thing")
            .expect("probe");
        let null = engine.null_index.expect("NULL");
        assert_eq!(
            engine.probes[thing].fallback_chain,
            vec![thing, null],
            "itself, then NULL"
        );
        // The NULL probe itself has nothing to fall back to.
        assert_eq!(engine.probes[null].fallback_chain, vec![null]);
    }

    #[test]
    fn the_fallback_directive_is_parsed_and_unknown_names_are_dropped() {
        let engine = Engine::parse(
            "Probe TCP A q|x|\nfallback B, Missing\nmatch a m|x| p/A/\n\
             Probe TCP B q|y|\nmatch b m|y| p/B/\n",
        );
        let a = engine.probes.iter().position(|p| p.name == "A").unwrap();
        let b = engine.probes.iter().position(|p| p.name == "B").unwrap();
        assert_eq!(engine.probes[a].fallback_names, vec!["B", "Missing"]);
        assert_eq!(
            engine.probes[a].fallback_chain,
            vec![a, b],
            "the unknown name is dropped, and there is no NULL probe"
        );
    }

    #[test]
    fn probes_are_offered_lowest_rarity_first() {
        let engine = Engine::parse(
            "Probe TCP Rare q|x|\nports 80\nrarity 9\nmatch r m|x| p/Rare/\n\
             Probe TCP Common q|y|\nports 80\nrarity 2\nmatch c m|y| p/Common/\n",
        );
        let names: Vec<&str> = engine
            .probes_for(80)
            .iter()
            .map(|probe| probe.name.as_str())
            .collect();
        assert_eq!(names[0], "Common", "rarity 2 before rarity 9");
    }

    #[test]
    fn an_ssl_only_probe_is_not_sent_in_the_clear() {
        let engine = Engine::parse(
            "Probe TCP SSLSessionReq q|\x16\x03|\nsslports 443\nmatch ssl m|x| p/SSL/\n",
        );
        assert!(
            engine
                .probes_for(443)
                .iter()
                .all(|probe| probe.name != "SSLSessionReq"),
            "an sslports-only probe must wait for TLS"
        );

        // A probe that names plaintext ports as well is still offered in the
        // clear, the way nmap's GetRequest names both `ports` and `sslports`.
        let engine = Engine::parse(
            "Probe TCP Both q|x|\nports 80\nsslports 443\nmatch b m|x| p/Both/\n",
        );
        assert!(
            engine
                .probes_for(80)
                .iter()
                .any(|probe| probe.name == "Both")
        );
    }

    #[test]
    fn udp_probes_and_junk_lines_are_skipped_without_failing() {
        let engine = Engine::parse(
            "Probe UDP Ntp q|\\x1b|\nmatch ntp m|*| p/NTP/\n\
             nonsense\n\
             Probe TCP Good q|x|\nmatch http m|HTTP*| p/HTTP/\n\
             match broken-without-delimiters\n",
        );
        assert_eq!(engine.len(), 1, "only the usable TCP probe");
        assert_eq!(engine.probes[0].name, "Good");
    }

    #[test]
    fn a_supplied_file_is_tried_before_the_built_in_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probes");
        std::fs::write(
            &path,
            "Probe TCP Mine q|hello|\nmatch mine m|hello| p/Mine/\n",
        )
        .unwrap();

        let engine = Engine::load(Some(&path));
        assert!(engine.len() > 1);
        assert_eq!(engine.probes[0].name, "Mine");
        assert!(engine.probes.iter().any(|probe| probe.name == "NULL"));

        // A path that does not exist falls back rather than failing the scan.
        let missing = Engine::load(Some(std::path::Path::new("/nonexistent/probes")));
        assert_eq!(missing.len(), Engine::embedded().len());
    }

    #[test]
    fn a_wildcard_does_not_run_away_with_a_pathological_pattern() {
        // Many stars against a long text: the step guard stops it, and the
        // answer is "no match" rather than a hang.
        let pattern = Pattern::parse(&format!("m|{}|", "*a".repeat(30))).expect("pattern");
        let text = "b".repeat(300);
        assert!(pattern.captures(&text).is_none());
    }

    /// The engine against a socket we control: no network, no external process.
    #[test]
    fn the_engine_identifies_a_service_over_a_real_socket() {
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).expect("non-blocking");

        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            // Like a real FTP server: greet whoever connects, without being
            // asked. That is exactly what the NULL probe is for.
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.write_all(b"220 ProFTPD 1.3.8 Server ready\r\n");
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });

        let found = engine().identify_port(
            "127.0.0.1".parse().unwrap(),
            port,
            Duration::from_millis(300),
        );
        stop.store(true, Ordering::Relaxed);
        server.join().ok();

        let found = found.expect("a service");
        assert_eq!(found.service, "ftp");
        assert_eq!(found.product.as_deref(), Some("ProFTPD"));
        assert_eq!(found.version.as_deref(), Some("1.3.8"));
        assert!(found.response.contains("ProFTPD"), "{}", found.response);
    }

    #[test]
    fn a_quiet_port_identifies_as_nothing() {
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // A listener that accepts and says nothing: every probe times out, and
        // the engine gives up rather than inventing a service.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).expect("non-blocking");

        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                if listener.accept().is_err() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });

        let found = engine().identify_port(
            "127.0.0.1".parse().unwrap(),
            port,
            Duration::from_millis(120),
        );
        stop.store(true, Ordering::Relaxed);
        server.join().ok();
        assert!(found.is_none(), "{found:?}");
    }
}
