//! Small glob matcher for search filters: `*` (within a component), `**`
//! (any number of components), `?` and `[...]` classes (`[!...]` negates).
//! A pattern without `/` matches the file name only.

/// Matches `pattern` against a slash-separated relative path.
#[must_use]
pub(crate) fn matches(pattern: &str, path: &str, case_sensitive: bool) -> bool {
    let (p, s) = if case_sensitive {
        (pattern.to_owned(), path.to_owned())
    } else {
        (pattern.to_lowercase(), path.to_lowercase())
    };

    if !p.contains('/') {
        let name = s.rsplit('/').next().unwrap_or(&s);
        return component(p.as_bytes(), name.as_bytes());
    }

    let pc: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
    let sc: Vec<&str> = s.split('/').filter(|c| !c.is_empty()).collect();

    components(&pc, &sc)
}

fn components(p: &[&str], s: &[&str]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((&"**", rest)) => (0..=s.len()).any(|i| components(rest, &s[i..])),
        Some((first, rest)) => {
            !s.is_empty() && component(first.as_bytes(), s[0].as_bytes()) && components(rest, &s[1..])
        }
    }
}

/// Iterative wildcard match with backtracking on the last `*`.
fn component(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);

    while si < s.len() {
        if pi < p.len() {
            match p[pi] {
                b'*' => {
                    star = Some(pi);
                    mark = si;
                    pi += 1;

                    continue;
                }
                b'?' => {
                    pi += 1;
                    si += 1;

                    continue;
                }
                b'[' => {
                    if let Some((ok, len)) = class(&p[pi..], s[si])
                        && ok
                    {
                        pi += len;
                        si += 1;

                        continue;
                    }
                }
                c if c == s[si] => {
                    pi += 1;
                    si += 1;

                    continue;
                }
                _ => {}
            }
        }

        match star {
            Some(st) => {
                pi = st + 1;
                mark += 1;
                si = mark;
            }
            None => return false,
        }
    }

    p[pi..].iter().all(|&b| b == b'*')
}

/// `[abc]`, `[a-z]`, `[!x]`: whether `ch` matches, and the class length.
fn class(p: &[u8], ch: u8) -> Option<(bool, usize)> {
    let end = p.iter().skip(1).position(|&b| b == b']')? + 1;
    let body = &p[1..end];
    let (neg, body) = match body.first() {
        Some(b'!' | b'^') => (true, &body[1..]),
        _ => (false, body),
    };

    let mut hit = false;
    let mut i = 0;

    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == b'-' {
            hit |= (body[i]..=body[i + 2]).contains(&ch);
            i += 3;
        } else {
            hit |= body[i] == ch;
            i += 1;
        }
    }

    Some((hit != neg, end + 1))
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn globs() {
        assert!(matches("*.conf", "etc/nginx/nginx.conf", true));
        assert!(!matches("*.conf", "etc/nginx/nginx.conf.bak", true));
        assert!(matches("*.CONF", "a/b.conf", false));
        assert!(matches("nginx/**/*.conf", "nginx/sites/a/x.conf", true));
        assert!(matches("**/node_modules/**", "web/node_modules/x/y.js", true));
        assert!(matches("log?.txt", "log1.txt", true));
        assert!(matches("[a-c]*.rs", "build.rs", true));
        assert!(!matches("[!a-c]*.rs", "build.rs", true));
        assert!(matches("*", "anything", true));
        assert!(!matches("src/*.rs", "src/a/b.rs", true));
    }
}
