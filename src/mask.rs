use crate::lang::Language;

/// Source with comments and string bodies replaced by spaces.
/// Every masked line keeps the exact width of the original, so columns and
/// brace counting survive; only the *content* of literals disappears.
/// Interpolated expressions (`${...}`, f-strings) stay readable: hiding them
/// would report live functions as dead.
pub struct Masked {
    pub lines: Vec<String>,
}

impl Masked {
    pub fn line(&self, idx: usize) -> &str {
        self.lines.get(idx).map(String::as_str).unwrap_or("")
    }
}

#[derive(Clone, Copy)]
enum State {
    Normal,
    LineComment,
    BlockComment { depth: usize },
    Quoted { quote: u8, triple: bool, multiline: bool, interp: Interp },
    Expr { quote: u8, triple: bool, multiline: bool, depth: usize, dollar: bool },
    Raw { hashes: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Interp {
    None,
    /// `${...}` — JavaScript template literals.
    Dollar,
    /// `{...}` — Python f-strings.
    Brace,
}

pub fn mask(source: &str, lang: Language) -> Masked {
    let bytes = source.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    let mut state = State::Normal;
    let mut i = 0usize;

    let line_marker = lang.line_comment().as_bytes();
    let block = lang.block_comment();

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            State::Normal => {
                if c == b'\n' {
                    out[i] = b'\n';
                    i += 1;
                    continue;
                }
                if starts(bytes, i, line_marker) {
                    state = State::LineComment;
                    i += line_marker.len();
                    continue;
                }
                if let Some((open, _)) = block {
                    if starts(bytes, i, open.as_bytes()) {
                        state = State::BlockComment { depth: 1 };
                        i += open.len();
                        continue;
                    }
                }
                if let Some((open_len, hashes)) = rust_raw_open(bytes, i) {
                    state = State::Raw { hashes };
                    i += open_len;
                    continue;
                }
                if let Some(q) = quote_open(bytes, i, lang) {
                    let triple = q.len() == 3;
                    let multiline = triple || q[0] == b'`';
                    let interp = if q[0] == b'`' {
                        Interp::Dollar
                    } else if lang == Language::Python && is_fstring(bytes, i) {
                        Interp::Brace
                    } else {
                        Interp::None
                    };
                    i += q.len();
                    state = State::Quoted { quote: q[q.len() - 1], triple, multiline, interp };
                    continue;
                }
                out[i] = c;
                i += 1;
            }
            State::LineComment => {
                if c == b'\n' {
                    out[i] = b'\n';
                    state = State::Normal;
                }
                i += 1;
            }
            State::BlockComment { depth } => {
                if let Some((open, close)) = block {
                    if starts(bytes, i, close.as_bytes()) {
                        i += close.len();
                        state = if depth == 1 {
                            State::Normal
                        } else {
                            State::BlockComment { depth: depth - 1 }
                        };
                        continue;
                    }
                    if starts(bytes, i, open.as_bytes()) {
                        state = State::BlockComment { depth: depth + 1 };
                        i += open.len();
                        continue;
                    }
                }
                if c == b'\n' {
                    out[i] = b'\n';
                }
                i += 1;
            }
            State::Quoted { quote, triple, multiline, interp } => {
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == b'\n' {
                    out[i] = b'\n';
                    if !multiline {
                        state = State::Normal;
                    }
                    i += 1;
                    continue;
                }
                match interp {
                    Interp::Dollar if c == b'$' && bytes.get(i + 1) == Some(&b'{') => {
                        out[i] = b'$';
                        out[i + 1] = b'{';
                        state = State::Expr { quote, triple, multiline, depth: 0, dollar: true };
                        i += 2;
                        continue;
                    }
                    Interp::Brace if c == b'{' && bytes.get(i + 1) != Some(&b'{') => {
                        out[i] = b'{';
                        state = State::Expr { quote, triple, multiline, depth: 0, dollar: false };
                        i += 1;
                        continue;
                    }
                    _ => {}
                }
                let closes = if triple {
                    starts(bytes, i, &[quote, quote, quote])
                } else {
                    c == quote
                };
                if closes {
                    state = State::Normal;
                    i += if triple { 3 } else { 1 };
                    continue;
                }
                i += 1;
            }
            State::Expr { quote, triple, multiline, depth, dollar } => {
                out[i] = c;
                let interp = if dollar { Interp::Dollar } else { Interp::Brace };
                match c {
                    b'{' => {
                        state = State::Expr {
                            quote,
                            triple,
                            multiline,
                            depth: depth + 1,
                            dollar,
                        };
                    }
                    b'}' if depth == 0 => {
                        state = State::Quoted {
                            quote,
                            triple,
                            multiline,
                            interp,
                        };
                    }
                    b'}' => {
                        state = State::Expr {
                            quote,
                            triple,
                            multiline,
                            depth: depth - 1,
                            dollar,
                        };
                    }
                    b'"' | b'\'' | b'`' => {
                        // Copy a nested literal verbatim so its braces cannot
                        // unbalance the surrounding expression.
                        let mut j = i + 1;
                        while j < bytes.len() && bytes[j] != c && bytes[j] != b'\n' {
                            out[j] = bytes[j];
                            if bytes[j] == b'\\' {
                                j += 1;
                                if j < bytes.len() {
                                    out[j] = bytes[j];
                                }
                            }
                            j += 1;
                        }
                        if j < bytes.len() && bytes[j] == c {
                            out[j] = c;
                        }
                        i = j + 1;
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
            State::Raw { hashes } => {
                if c == b'\n' {
                    out[i] = b'\n';
                }
                if c == b'"' && followed_by_hashes(bytes, i + 1, hashes) {
                    state = State::Normal;
                    i += 1 + hashes;
                    continue;
                }
                i += 1;
            }
        }
    }

    let text = String::from_utf8(out).unwrap_or_else(|_| source.to_string());
    Masked { lines: text.lines().map(|l| l.to_string()).collect() }
}

fn starts(hay: &[u8], at: usize, needle: &[u8]) -> bool {
    hay.len() >= at + needle.len() && hay[at..at + needle.len()] == *needle
}

/// `r"..."` / `r#"..."#` / `br#"..."#` in Rust only when not glued to an identifier.
/// Returns (length of the opening sequence, number of hashes).
fn rust_raw_open(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let prev_ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
    if !prev_ok {
        return None;
    }
    let mut p = i;
    if bytes.get(p) == Some(&b'b') {
        p += 1;
    }
    if bytes.get(p) != Some(&b'r') {
        return None;
    }
    p += 1;
    let mut hashes = 0;
    while bytes.get(p) == Some(&b'#') {
        hashes += 1;
        p += 1;
    }
    if bytes.get(p) == Some(&b'"') {
        Some((p + 1 - i, hashes))
    } else {
        None
    }
}

fn followed_by_hashes(bytes: &[u8], mut at: usize, hashes: usize) -> bool {
    for _ in 0..hashes {
        if bytes.get(at) != Some(&b'#') {
            return false;
        }
        at += 1;
    }
    true
}

fn is_fstring(bytes: &[u8], quote_at: usize) -> bool {
    let mut back = 0;
    while back < 3 && quote_at > back {
        let c = bytes[quote_at - back - 1];
        match c {
            b'f' | b'F' => return true,
            b'r' | b'R' | b'b' | b'B' | b'u' | b'U' => back += 1,
            _ => return false,
        }
    }
    false
}

/// Returns the delimiter of a string/char literal opening at `i`, if any.
fn quote_open(bytes: &[u8], i: usize, lang: Language) -> Option<&'static [u8]> {
    let c = *bytes.get(i)?;
    if c == b'"' {
        if lang == Language::Python && starts(bytes, i, b"\"\"\"") {
            return Some(b"\"\"\"");
        }
        return Some(b"\"");
    }
    if c == b'`' && matches!(lang, Language::JavaScript | Language::TypeScript) {
        return Some(b"`");
    }
    if c == b'\'' {
        if lang == Language::Python && starts(bytes, i, b"'''") {
            return Some(b"'''");
        }
        if lang == Language::Python {
            return Some(b"'");
        }
        // Rust/Go/JS: only real char literals, never lifetimes or apostrophes.
        let rest = bytes.get(i + 1..)?;
        return match rest {
            [_, b'\'', ..] => Some(b"'"),
            [b'\\', _, b'\'', ..] => Some(b"'"),
            _ => None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_comments_lose_their_code() {
        let src = "fn main() {\n    // fn fake() {}\n    let s = \"fn fake2() {}\";\n}\n";
        let m = mask(src, Language::Rust);
        assert!(!m.lines.join("\n").contains("fake"));
        assert!(m.lines[0].contains("fn main"));
        assert_eq!(m.lines.len(), src.lines().count());
    }

    #[test]
    fn widths_are_preserved() {
        let src = "let a = \"xy\"; // note\nlet b = 2;\n";
        let m = mask(src, Language::Rust);
        for (orig, masked) in src.lines().zip(m.lines.iter()) {
            assert_eq!(orig.len(), masked.len());
        }
    }

    #[test]
    fn template_interpolation_is_kept() {
        let src = "const s = `hello ${normalize(name)}`;\n";
        let m = mask(src, Language::JavaScript);
        assert!(m.lines[0].contains("normalize"), "got: {}", m.lines[0]);
    }

    #[test]
    fn fstring_interpolation_is_kept() {
        let src = "text = f\"<p>{normalize(value)}</p>\"\n";
        let m = mask(src, Language::Python);
        assert!(m.lines[0].contains("normalize"), "got: {}", m.lines[0]);
    }

    #[test]
    fn plain_strings_hide_names() {
        let src = "let s = \"ghost_two\";\n";
        let m = mask(src, Language::Rust);
        assert!(!m.lines[0].contains("ghost_two"));
    }

    #[test]
    fn lifetimes_are_not_char_literals() {
        let src = "fn get<'a>(x: &'a str) -> &'a str { x }\n";
        let m = mask(src, Language::Rust);
        assert!(m.lines[0].contains("'a"));
    }
}
