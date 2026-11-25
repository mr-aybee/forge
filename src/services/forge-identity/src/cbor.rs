//! A strict, minimal CBOR reader for WebAuthn (RFC 8949 subset).
//!
//! Accepts only what WebAuthn uses: unsigned and negative integers, byte
//! strings, text strings, arrays, maps, and the simple values false, true and
//! null, all with definite lengths. Rejects tags, floats, indefinite lengths,
//! non-shortest encodings are tolerated (authenticators vary) but every length
//! is bounded, nesting is limited, and malformed input is an error, never a
//! panic: every read is bounds-checked.

/// Deepest nesting accepted (WebAuthn needs 3).
pub const MAX_DEPTH: usize = 8;
/// Largest byte or text string accepted.
pub const MAX_BYTES: usize = 16 * 1024;
/// Most entries in one array or map.
pub const MAX_ITEMS: usize = 64;

/// A decoded CBOR value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Unsigned or negative integer (CBOR major types 0 and 1).
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    /// Entries in encoded order; keys are unique.
    Map(Vec<(Value, Value)>),
    Bool(bool),
    Null,
}

impl Value {
    /// Looks up a map entry by a text key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Map(m) => m.iter().find(|(k, _)| matches!(k, Self::Text(t) if t == key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Looks up a map entry by an integer key (COSE keys).
    #[must_use]
    pub fn get_int(&self, key: i128) -> Option<&Value> {
        match self {
            Self::Map(m) => m.iter().find(|(k, _)| matches!(k, Self::Int(i) if *i == key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The bytes of a byte string.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The text of a text string.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The value of an integer.
    #[must_use]
    pub const fn as_int(&self) -> Option<i128> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// A CBOR decoding error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid CBOR: {0}")]
pub struct CborError(pub &'static str);

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, CborError> {
        let b = *self.data.get(self.pos).ok_or(CborError("truncated"))?;
        self.pos += 1;

        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&[u8], CborError> {
        let end = self.pos.checked_add(n).ok_or(CborError("length overflow"))?;
        let s = self.data.get(self.pos..end).ok_or(CborError("truncated"))?;
        self.pos = end;

        Ok(s)
    }

    /// The argument of a head: the value of `info` (0-23) or the following
    /// 1, 2, 4 or 8 bytes. Indefinite lengths (31) and reserved values are
    /// rejected.
    fn argument(&mut self, info: u8) -> Result<u64, CborError> {
        Ok(match info {
            0..=23 => u64::from(info),
            24 => u64::from(self.byte()?),
            25 => u64::from(u16::from_be_bytes(self.take(2)?.try_into().map_err(|_| CborError("truncated"))?)),
            26 => u64::from(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| CborError("truncated"))?)),
            27 => u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| CborError("truncated"))?),
            31 => return Err(CborError("indefinite lengths are not allowed")),
            _ => return Err(CborError("reserved additional information")),
        })
    }

    fn len(&mut self, info: u8, max: usize) -> Result<usize, CborError> {
        let n = self.argument(info)?;
        let n = usize::try_from(n).map_err(|_| CborError("length too large"))?;
        if n > max {
            return Err(CborError("length over the limit"));
        }

        Ok(n)
    }

    fn value(&mut self, depth: usize) -> Result<Value, CborError> {
        if depth > MAX_DEPTH {
            return Err(CborError("nested too deeply"));
        }

        let head = self.byte()?;
        let (major, info) = (head >> 5, head & 0x1f);
        match major {
            0 => Ok(Value::Int(i128::from(self.argument(info)?))),
            1 => Ok(Value::Int(-1 - i128::from(self.argument(info)?))),
            2 => {
                let n = self.len(info, MAX_BYTES)?;
                Ok(Value::Bytes(self.take(n)?.to_vec()))
            }
            3 => {
                let n = self.len(info, MAX_BYTES)?;
                let s = std::str::from_utf8(self.take(n)?).map_err(|_| CborError("text is not UTF-8"))?;
                Ok(Value::Text(s.to_owned()))
            }
            4 => {
                let n = self.len(info, MAX_ITEMS)?;
                let mut items = Vec::with_capacity(n);

                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }

                Ok(Value::Array(items))
            }
            5 => {
                let n = self.len(info, MAX_ITEMS)?;
                let mut entries: Vec<(Value, Value)> = Vec::with_capacity(n);

                for _ in 0..n {
                    let k = self.value(depth + 1)?;
                    if !matches!(k, Value::Int(_) | Value::Text(_)) {
                        return Err(CborError("map keys must be integers or text"));
                    }

                    if entries.iter().any(|(e, _)| *e == k) {
                        return Err(CborError("duplicate map key"));
                    }

                    let v = self.value(depth + 1)?;
                    entries.push((k, v));
                }

                Ok(Value::Map(entries))
            }
            6 => Err(CborError("tags are not allowed")),
            _ => match info {
                20 => Ok(Value::Bool(false)),
                21 => Ok(Value::Bool(true)),
                22 => Ok(Value::Null),
                _ => Err(CborError("floats and other simple values are not allowed")),
            },
        }
    }
}

/// Decodes exactly one item filling the whole input (no trailing bytes).
pub fn decode(data: &[u8]) -> Result<Value, CborError> {
    let (v, used) = decode_prefix(data)?;
    if used != data.len() {
        return Err(CborError("trailing bytes"));
    }

    Ok(v)
}

/// Decodes one item at the start of `data`; returns it and the bytes used
/// (the credential public key inside authenticator data is followed by
/// optional extensions).
pub fn decode_prefix(data: &[u8]) -> Result<(Value, usize), CborError> {
    let mut r = Reader { data, pos: 0 };
    let v = r.value(0)?;

    Ok((v, r.pos))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_webauthn_subset() {
        // {"fmt": "none", "attStmt": {}, "authData": h'0102'}
        let mut doc = vec![0xa3, 0x63];
        doc.extend(b"fmt");
        doc.push(0x64);
        doc.extend(b"none");
        doc.push(0x67);
        doc.extend(b"attStmt");
        doc.push(0xa0);
        doc.push(0x68);
        doc.extend(b"authData");
        doc.extend([0x42, 1, 2]);
        let v = decode(&doc).unwrap();
        assert_eq!(v.get("fmt").and_then(Value::as_text), Some("none"));
        assert_eq!(v.get("authData").and_then(Value::as_bytes), Some(&[1u8, 2][..]));

        // COSE-style integer keys, negative numbers.
        let cose = [0xa2, 0x01, 0x02, 0x03, 0x26];
        let k = decode(&cose).unwrap();
        assert_eq!(k.get_int(1).and_then(Value::as_int), Some(2));
        assert_eq!(k.get_int(3).and_then(Value::as_int), Some(-7));
    }

    #[test]
    fn rejects_what_webauthn_never_needs() {
        for bad in [
            &[0x5f, 0x41, 0x00, 0xff][..],   // indefinite byte string
            &[0xc0, 0x00],                   // tag
            &[0xf9, 0x3c, 0x00],             // half float
            &[0xa2, 0x01, 0x00, 0x01, 0x00], // duplicate key
            &[0xa1, 0x41, 0x00, 0x00],       // byte-string key
            &[0x42, 0x01],                   // truncated
            &[0x01, 0x02],                   // trailing bytes
            &[0x63, 0xff, 0xfe, 0xfd],       // invalid UTF-8
            &[0x1c],                         // reserved
        ] {
            assert!(decode(bad).is_err(), "{bad:02x?}");
        }

        // Lengths beyond the limits fail before allocating.
        assert!(decode(&[0x5a, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());

        // Nesting limit.
        let deep = [0x81u8; 32];
        assert!(decode(&deep).is_err());
    }

    /// Fuzz-style: random and mutated inputs never panic (deterministic
    /// xorshift generator, no extra dependency).
    #[test]
    fn never_panics_on_malformed_input() {
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let seed = [0xa3u8, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e', 0x01, 0x26, 0x58, 0x20];

        for i in 0..20_000 {
            let len = usize::try_from(next() % 64).unwrap();

            let mut buf: Vec<u8> = if i % 2 == 0 {
                (0..len).map(|_| u8::try_from(next() & 0xff).unwrap()).collect()
            } else {
                let mut b = seed.to_vec();

                for _ in 0..3 {
                    let at = usize::try_from(next()).unwrap() % b.len();
                    b[at] = u8::try_from(next() & 0xff).unwrap();
                }

                b
            };

            buf.truncate(len.max(1).min(buf.len()));
            let _ = decode(&buf);
            let _ = decode_prefix(&buf);
        }
    }
}
