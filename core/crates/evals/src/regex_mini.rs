//! 够用就好的正则子集（`check: text` 的 `matches` 与 `check: distinct_matches` 的
//! `pattern` 吃它）。
//!
//! workspace 没钉 `regex`，加依赖要停下报告（spec §7.1 第 3 条），所以这里自带一个
//! 回溯匹配器。覆盖场景文件与断言 DSL 实际用到的语法：
//!
//! * 字面字符、`.`
//! * 字符类 `[a-z0-9_]` / `[^…]`，以及 `\d \D \w \W \s \S`
//! * 量词 `* + ? {m} {m,} {m,n}`（都是贪婪的）
//! * 分组 `( … )` 与交替 `|`
//! * 锚 `^` `$`
//!
//! 不支持的语法（反向引用、前后瞻、非贪婪 `*?`、命名组）一律在编译期报错，
//! 而不是静默匹配错 —— 断言写错了要当场知道。
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexError(pub String);

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone)]
enum ClassItem {
    One(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Debug, Clone)]
struct CharClass {
    negated: bool,
    items: Vec<ClassItem>,
}

impl CharClass {
    fn matches(&self, c: char) -> bool {
        let hit = self.items.iter().any(|item| match item {
            ClassItem::One(x) => c == *x,
            ClassItem::Range(a, b) => c >= *a && c <= *b,
            ClassItem::Digit(want) => c.is_ascii_digit() == *want,
            ClassItem::Word(want) => (c.is_alphanumeric() || c == '_') == *want,
            ClassItem::Space(want) => c.is_whitespace() == *want,
        });
        hit != self.negated
    }
}

#[derive(Debug, Clone)]
enum Atom {
    Char(char),
    Any,
    Class(CharClass),
    Group(Vec<Vec<Piece>>),
    Start,
    End,
}

#[derive(Debug, Clone)]
struct Piece {
    atom: Atom,
    min: usize,
    max: Option<usize>,
}

/// 编译好的模式。
#[derive(Debug, Clone)]
pub struct Regex {
    alts: Vec<Vec<Piece>>,
    source: String,
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Self, RegexError> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser { chars, pos: 0 };
        let alts = p.parse_alternation()?;
        if p.pos != p.chars.len() {
            return Err(RegexError(format!(
                "第 {} 个字符起看不懂：{:?}",
                p.pos,
                p.chars[p.pos..].iter().collect::<String>()
            )));
        }
        Ok(Regex {
            alts,
            source: pattern.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// 有没有一处匹配（相当于 Python 的 `re.search`）。
    pub fn is_match(&self, text: &str) -> bool {
        self.find_at(&text.chars().collect::<Vec<_>>(), 0).is_some()
    }

    /// 所有非重叠匹配的文本（相当于 Python 的 `re.findall`，模式里无分组时）。
    pub fn find_all(&self, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i <= chars.len() {
            match self.find_at(&chars, i) {
                None => i += 1,
                Some((start, end)) => {
                    out.push(chars[start..end].iter().collect());
                    i = if end > start { end } else { start + 1 };
                }
            }
        }
        out
    }

    /// 从 `from` 起往后找第一处匹配，返回 (起, 止)。
    fn find_at(&self, chars: &[char], from: usize) -> Option<(usize, usize)> {
        for start in from..=chars.len() {
            for alt in &self.alts {
                if let Some(end) = seq_match(alt, chars, start) {
                    return Some((start, end));
                }
            }
        }
        None
    }
}

fn seq_match(pieces: &[Piece], s: &[char], i: usize) -> Option<usize> {
    let Some((p, rest)) = pieces.split_first() else {
        return Some(i);
    };
    rep_match(p, rest, s, i, 0)
}

/// 一条序列在位置 `i` 上**所有**可能的结束位置（顶层只要第一个，组里要全部）。
fn seq_match_all(pieces: &[Piece], s: &[char], i: usize) -> Vec<usize> {
    let Some((p, rest)) = pieces.split_first() else {
        return vec![i];
    };
    rep_match_all(p, rest, s, i, 0)
}

fn rep_match_all(p: &Piece, rest: &[Piece], s: &[char], i: usize, done: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if p.max.is_none_or(|m| done < m) {
        for next in atom_match(&p.atom, s, i) {
            if next == i && done >= p.min {
                continue;
            }
            out.extend(rep_match_all(p, rest, s, next, done + 1));
        }
    }
    if done >= p.min {
        out.extend(seq_match_all(rest, s, i));
    }
    out
}

fn rep_match(p: &Piece, rest: &[Piece], s: &[char], i: usize, done: usize) -> Option<usize> {
    // 贪婪：先试着再匹配一次
    if p.max.is_none_or(|m| done < m) {
        for next in atom_match(&p.atom, s, i) {
            // 空匹配（比如 `(a*)*`）配够下限之后就停，免得原地打转
            if next == i && done >= p.min {
                continue;
            }
            if let Some(end) = rep_match(p, rest, s, next, done + 1) {
                return Some(end);
            }
        }
    }
    if done >= p.min {
        return seq_match(rest, s, i);
    }
    None
}

fn atom_match(atom: &Atom, s: &[char], i: usize) -> Vec<usize> {
    match atom {
        Atom::Char(c) => {
            if s.get(i) == Some(c) {
                vec![i + 1]
            } else {
                vec![]
            }
        }
        Atom::Any => {
            // Python 的 `.` 默认不匹配换行（没开 re.DOTALL）。
            if s.get(i).is_some_and(|c| *c != '\n') {
                vec![i + 1]
            } else {
                vec![]
            }
        }
        Atom::Class(cc) => {
            if s.get(i).is_some_and(|c| cc.matches(*c)) {
                vec![i + 1]
            } else {
                vec![]
            }
        }
        Atom::Group(alts) => {
            // 必须给出**所有**可能的结束位置：只给贪婪那一个的话，
            // `(a*)ab` 对 "aab" 会失败 —— 组里的 `a*` 吃光了 a 就退不回来。
            let mut ends: Vec<usize> = alts
                .iter()
                .flat_map(|alt| seq_match_all(alt, s, i))
                .collect();
            ends.sort_unstable_by(|a, b| b.cmp(a)); // 长的先试，保持贪婪语义
            ends.dedup();
            ends
        }
        Atom::Start => {
            if i == 0 {
                vec![i]
            } else {
                vec![]
            }
        }
        Atom::End => {
            // Python 的 `$` 除了串尾，也匹配「末尾换行之前」——
            // YAML 的 `|` 块标量天然带一个尾换行（05 的 reply 就是），
            // 不认这条的话 `x$` 对 "x\n" 会假红。
            if i == s.len() || (i + 1 == s.len() && s[i] == '\n') {
                vec![i]
            } else {
                vec![]
            }
        }
    }
}

// --------------------------------------------------------------------------

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn parse_alternation(&mut self) -> Result<Vec<Vec<Piece>>, RegexError> {
        let mut alts = vec![self.parse_sequence()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            alts.push(self.parse_sequence()?);
        }
        Ok(alts)
    }

    fn parse_sequence(&mut self) -> Result<Vec<Piece>, RegexError> {
        let mut pieces = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.parse_atom()?;
            let (min, max) = self.parse_quantifier(&atom)?;
            pieces.push(Piece { atom, min, max });
        }
        Ok(pieces)
    }

    fn parse_atom(&mut self) -> Result<Atom, RegexError> {
        match self.bump() {
            None => Err(RegexError("模式提前结束".to_string())),
            Some('^') => Ok(Atom::Start),
            Some('$') => Ok(Atom::End),
            Some('.') => Ok(Atom::Any),
            Some('(') => {
                // `(?:` 放行（只分组不捕获，对本引擎无害）；别的 `(?…` 仍然拒。
                if self.peek() == Some('?') {
                    self.bump();
                    if self.bump() != Some(':') {
                        return Err(RegexError("只支持 (?:…) 这一种扩展分组".to_string()));
                    }
                } else {
                    // 裸 `(` 是捕获分组。Python 的 findall 在有分组时返回的是**分组**
                    // 而不是整段匹配，本引擎没有捕获，静默返回整段就是错判
                    // （distinct_matches 的计数会偏）。要求显式写成非捕获。
                    return Err(RegexError(
                        "不支持捕获分组：findall 在有分组时返回的是分组而不是整段匹配，                         这里做不到。只想分组请写 (?:…)"
                            .to_string(),
                    ));
                }
                let alts = self.parse_alternation()?;
                if self.bump() != Some(')') {
                    return Err(RegexError("括号没闭合".to_string()));
                }
                Ok(Atom::Group(alts))
            }
            Some('[') => self.parse_class(),
            Some('\\') => self.parse_escape(),
            Some(c @ ('*' | '+' | '?')) => {
                Err(RegexError(format!("量词 {c:?} 前面没有可重复的东西")))
            }
            Some(c) => Ok(Atom::Char(c)),
        }
    }

    fn parse_escape(&mut self) -> Result<Atom, RegexError> {
        let c = self
            .bump()
            .ok_or_else(|| RegexError("反斜杠后面没东西".to_string()))?;
        let class = |item: ClassItem| {
            Ok(Atom::Class(CharClass {
                negated: false,
                items: vec![item],
            }))
        };
        match c {
            'd' => class(ClassItem::Digit(true)),
            'D' => class(ClassItem::Digit(false)),
            'w' => class(ClassItem::Word(true)),
            'W' => class(ClassItem::Word(false)),
            's' => class(ClassItem::Space(true)),
            'S' => class(ClassItem::Space(false)),
            'n' => Ok(Atom::Char('\n')),
            't' => Ok(Atom::Char('\t')),
            'r' => Ok(Atom::Char('\r')),
            '1'..='9' => Err(RegexError("不支持反向引用".to_string())),
            // 字母类转义一律走白名单。原来是 `other => Char(other)` 兜底，于是
            // `\b`（词边界）被当成字母 b、`\A`/`\Z`（串锚）被当成 A/Z ——
            // 断言静默匹配错，而模块头承诺的是「不支持的语法在编译期报错」。
            c if c.is_ascii_alphanumeric() => Err(RegexError(format!(
                "不支持 \\{c} 这个转义（词边界 \\b、串锚 \\A \\Z 都没实现）"
            ))),
            other => Ok(Atom::Char(other)),
        }
    }

    fn parse_class(&mut self) -> Result<Atom, RegexError> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = self
                .bump()
                .ok_or_else(|| RegexError("字符类 [ 没闭合".to_string()))?;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                match self.parse_escape()? {
                    Atom::Char(x) => x,
                    Atom::Class(cc) => {
                        items.extend(cc.items);
                        continue;
                    }
                    _ => return Err(RegexError("字符类里出现了不该有的转义".to_string())),
                }
            } else {
                c
            };
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|n| *n != ']') {
                self.pos += 1;
                let hi = self
                    .bump()
                    .ok_or_else(|| RegexError("字符类的区间缺上界".to_string()))?;
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::One(lo));
            }
        }
        if items.is_empty() {
            return Err(RegexError("空的字符类 []".to_string()));
        }
        Ok(Atom::Class(CharClass { negated, items }))
    }

    fn parse_quantifier(&mut self, atom: &Atom) -> Result<(usize, Option<usize>), RegexError> {
        let anchor = matches!(atom, Atom::Start | Atom::End);
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => {
                let save = self.pos;
                match self.parse_braces() {
                    Ok(v) => v,
                    Err(_) => {
                        self.pos = save; // `{` 不是量词时当普通字符（Python 也这么办）
                        (1, Some(1))
                    }
                }
            }
            _ => (1, Some(1)),
        };
        if self.peek() == Some('?') && (min, max) != (1, Some(1)) {
            return Err(RegexError("不支持非贪婪量词".to_string()));
        }
        if anchor && (min, max) != (1, Some(1)) {
            return Err(RegexError("锚点不能带量词".to_string()));
        }
        Ok((min, max))
    }

    fn parse_braces(&mut self) -> Result<(usize, Option<usize>), RegexError> {
        let start = self.pos;
        debug_assert_eq!(self.chars.get(start), Some(&'{'));
        self.pos += 1;
        let mut body = String::new();
        loop {
            match self.bump() {
                None => return Err(RegexError("{ 没闭合".to_string())),
                Some('}') => break,
                Some(c) => body.push(c),
            }
        }
        let parse = |s: &str| s.trim().parse::<usize>().ok();
        match body.split_once(',') {
            None => {
                let n = parse(&body).ok_or_else(|| RegexError("{n} 里不是数字".to_string()))?;
                Ok((n, Some(n)))
            }
            Some((lo, hi)) => {
                let min =
                    parse(lo).ok_or_else(|| RegexError("{m,n} 的下界不是数字".to_string()))?;
                if hi.trim().is_empty() {
                    return Ok((min, None));
                }
                let max =
                    parse(hi).ok_or_else(|| RegexError("{m,n} 的上界不是数字".to_string()))?;
                Ok((min, Some(max)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(p: &str) -> Regex {
        Regex::new(p).unwrap_or_else(|e| panic!("编译 {p:?} 失败：{e}"))
    }

    /// 场景里真正在用的那一个（05_history_summary 的 om_h[0-9]+），先保住它。
    #[test]
    fn the_pattern_the_suite_actually_uses() {
        let r = re("om_h[0-9]+");
        assert_eq!(
            r.find_all("引用 [om_h1] 与 [om_h3]，还有 [om_h5]"),
            vec!["om_h1", "om_h3", "om_h5"]
        );
        assert!(r.is_match("om_h42"));
        assert!(!r.is_match("om_hx"));
    }

    // --- 五类曾经静默错判的 ---------------------------------------------------

    /// `.` 不匹配换行（Python 没开 DOTALL）。曾经 Atom::Any 只判 `i < s.len()`。
    #[test]
    fn dot_does_not_cross_a_newline() {
        assert!(!re("a.b").is_match("a\nb"));
        assert!(re("a.b").is_match("axb"));
        assert_eq!(re("a.").find_all("a\nax"), vec!["ax"]);
    }

    /// `$` 认末尾那个换行。YAML 的 `|` 块标量天然带尾换行，不认这条会假红。
    #[test]
    fn dollar_accepts_one_trailing_newline() {
        assert!(re("x$").is_match("x"));
        assert!(re("x$").is_match("x\n"), "YAML 块标量的尾换行会让断言假红");
        assert!(!re("x$").is_match("x\n\n"));
        assert!(!re("x$").is_match("xy"));
    }

    /// 组内量词要能回溯。曾经 Group 只取贪婪那一个结束位置。
    #[test]
    fn a_group_backtracks_its_quantifier() {
        assert!(re("(?:a*)ab").is_match("aab"), "组里的 a* 要让得出一个 a");
        assert!(
            re("(?:a|ab)c").is_match("abc"),
            "第一个分支不行要退回去试第二个"
        );
        assert!(re("(?:ab?)+c").is_match("aababc"));
    }

    /// 词边界与串锚没实现 —— 必须编译期报错，不能当成普通字母。
    #[test]
    fn unimplemented_letter_escapes_are_rejected_not_silently_matched() {
        for p in [r"\bword", r"\Bx", r"\Atext", r"text\Z", r"\Gx"] {
            let err = Regex::new(p).expect_err(&format!("{p} 该被拒"));
            assert!(err.0.contains("不支持"), "{p}: {}", err.0);
        }
        // 曾经的错判形态：\b 被当成字母 b
        assert!(Regex::new(r"\bword").is_err());
    }

    /// 捕获分组要拒（Python findall 有分组时返回分组，本引擎给不出），
    /// 但 (?:…) 放行。
    #[test]
    fn capturing_groups_are_rejected_but_non_capturing_work() {
        let err = Regex::new("(ab)+").expect_err("捕获分组该被拒");
        assert!(err.0.contains("(?:"), "报错要告诉人怎么改：{}", err.0);
        assert!(re("(?:ab)+").is_match("abab"));
        assert!(Regex::new("(?=x)").is_err(), "前瞻仍然要拒");
        assert!(Regex::new("(?P<n>x)").is_err(), "命名组仍然要拒");
    }

    // --- 原本就该有的基本盘 ---------------------------------------------------

    #[test]
    fn anchors_classes_and_quantifiers() {
        assert!(re("^abc$").is_match("abc"));
        assert!(!re("^abc$").is_match("xabc"));
        assert!(re(r"\d{3}").is_match("x123"));
        assert!(!re(r"\d{3}").is_match("x12"));
        assert!(re("[^a-z]+").is_match("ABC"));
        assert!(re(r"a\.b").is_match("a.b"));
        assert!(!re(r"a\.b").is_match("axb"));
        // 实测 python3：findall("a{2,3}", "aaaa") == ["aaa"] —— 剩下那个 a 配不够 min=2
        assert_eq!(re("a{2,3}").find_all("aaaa"), vec!["aaa"]);
    }

    #[test]
    fn find_all_is_non_overlapping_and_handles_empty_matches() {
        assert_eq!(re("aa").find_all("aaaa"), vec!["aa", "aa"]);
        // 空匹配不能原地打转
        assert_eq!(re("a*").find_all("b").len(), 2, "每个位置一个空匹配");
    }

    #[test]
    fn malformed_patterns_report_instead_of_panicking() {
        // 注意 "a{2," 不在此列：Python 对没闭合的 { 是当字面量处理的，不报错。
        for p in ["(?:ab", "[a-", r"\1", "*abc"] {
            assert!(Regex::new(p).is_err(), "{p} 该报错");
        }
    }

    /// 非 ASCII 按 char 走，不能在字节边界上出事。
    #[test]
    fn works_on_multibyte_text() {
        assert!(re("中.$").is_match("中文"));
        assert_eq!(
            re("[\u{4e00}-\u{9fff}]+").find_all("ab中文cd汉"),
            vec!["中文", "汉"]
        );
    }
}
