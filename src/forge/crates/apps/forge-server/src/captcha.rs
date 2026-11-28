//! Image CAPTCHA for password sign-in (D-064, T01): after a few failed
//! attempts for a sign-in name or from an address, the next sign-in also
//! needs the six characters shown in a server-drawn picture.
//!
//! * Self-hosted: the server draws the characters as warped strokes into a
//!   grayscale bitmap and encodes it as PNG itself, so the text is never in
//!   the page (no `<text>`, no font, no third-party service).
//! * The alphabet leaves out look-alikes (`0/O`, `1/I/L`, `2/Z`, `5/S`, `8/B`).
//! * Only an HMAC of the answer is kept (random per-process key), bound to
//!   the client address it was issued to; single use, two minutes.
//! * Failures count per sign-in name AND per address (loopback included),
//!   so switching names does not skip it. The proof of work and the
//!   per-account throttle still apply in front of it.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ring::hmac;

/// Characters a challenge is made of: no look-alikes.
pub const ALPHABET: &[u8] = b"ACDEFGHJKMNPQRTUVWXY34679";
/// Characters per challenge.
pub const LEN: usize = 6;
/// How long a challenge can be answered.
pub const TTL: Duration = Duration::from_secs(120);
/// Failures count for this long.
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Outstanding challenges kept at most; beyond it the oldest are dropped
/// (never a refusal for everyone: that would let a flood lock users out).
const MAX_PENDING: usize = 10_000;
/// Failure counters and per-address issue counters kept at most; beyond it
/// the oldest are dropped, so the maps cannot grow without bound.
const MAX_COUNTERS: usize = 50_000;
/// Images one address (an IPv6 /64) may fetch per [`FAILURE_WINDOW`].
const MAX_ISSUED_PER_IP: u32 = 60;
/// Picture size in pixels.
pub const WIDTH: usize = 220;
pub const HEIGHT: usize = 80;

/// Why an answer was refused. Either way the challenge is used up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Unknown, used, expired or issued to another address.
    Invalid,
    /// The characters do not match.
    Wrong,
}

#[derive(Debug, Clone)]
pub struct Issued {
    pub id: String,
    /// The PNG bytes.
    pub png: Vec<u8>,
    pub expires_in: Duration,
}

#[derive(Debug)]
struct Pending {
    answer: hmac::Tag,
    ip: IpAddr,
    at: Instant,
}

#[derive(Debug, Default)]
struct Inner {
    pending: HashMap<String, Pending>,
    failures: HashMap<String, (u32, Instant)>,
    issued: HashMap<IpAddr, (u32, Instant)>,
    /// When `failures` was last pruned of expired counts.
    pruned: Option<Instant>,
}

/// Removes the oldest entries of `map` until it holds fewer than `cap`.
fn make_room<K: Clone + Eq + std::hash::Hash, V>(map: &mut HashMap<K, V>, cap: usize, at: impl Fn(&V) -> Instant) {
    while map.len() >= cap {
        let Some(oldest) = map.iter().min_by_key(|(_, v)| at(v)).map(|(k, _)| k.clone()) else { return };
        map.remove(&oldest);
    }
}

/// The sign-in CAPTCHA state (in memory; a restart forgets it, which only
/// lets a few more attempts through without a picture).
pub struct Captchas {
    key: hmac::Key,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Captchas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Captchas")
    }
}

impl Default for Captchas {
    fn default() -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, &forge_secrets::crypto::random_bytes::<32>()),
            inner: Mutex::new(Inner::default()),
        }
    }
}

/// A fixed-size key for a sign-in name (any length comes in).
fn name_key(login: &str) -> String {
    let hex = forge_secrets::crypto::sha256_hex(login.trim().to_lowercase().as_bytes());
    format!("n:{}", &hex[..32])
}

/// The address a client is counted and bound as: IPv4 as is, IPv6 by its
/// /64 (one subscriber usually holds a whole /64).
fn bucket(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let bits = u128::from(v6) & !((1u128 << 64) - 1);
            IpAddr::V6(std::net::Ipv6Addr::from(bits))
        }
        v4 => v4,
    }
}

fn ip_key(ip: IpAddr) -> String {
    format!("a:{}", bucket(ip))
}

/// Uppercase, without spaces: what the user typed, as compared.
fn normalize(answer: &str) -> String {
    answer.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase()
}

impl Inner {
    /// Drops expired failure counts, at most every 10 seconds (a full scan).
    fn prune(&mut self) {
        if self.pruned.is_none_or(|p| p.elapsed() >= Duration::from_secs(10)) {
            self.failures.retain(|_, (_, at)| at.elapsed() < FAILURE_WINDOW);
            self.pruned = Some(Instant::now());
        }
    }

    fn count(&mut self, k: &str) -> u32 {
        self.prune();
        self.failures.get(k).filter(|(_, at)| at.elapsed() < FAILURE_WINDOW).map_or(0, |(n, _)| *n)
    }
}

impl Captchas {
    fn tag(&self, id: &str, answer: &str) -> hmac::Tag {
        hmac::sign(&self.key, format!("{id}\n{}", normalize(answer)).as_bytes())
    }

    /// Whether a sign-in as `login` from `ip` needs a CAPTCHA: `threshold`
    /// or more recent failures for the name or the address (0 = never).
    #[must_use]
    pub fn required(&self, login: Option<&str>, ip: IpAddr, threshold: u32) -> bool {
        if threshold == 0 {
            return false;
        }

        let Ok(mut m) = self.inner.lock() else { return true };
        let by_ip = m.count(&ip_key(ip));
        let by_name = login.filter(|l| !l.trim().is_empty()).map_or(0, |l| m.count(&name_key(l)));

        by_ip.max(by_name) >= threshold
    }

    /// A new picture for `ip`. `None` when this address (IPv6: its /64)
    /// fetched too many. A full store drops its oldest pictures instead of
    /// refusing, so a flood cannot lock other users out.
    pub fn issue(&self, ip: IpAddr) -> Option<Issued> {
        let text = random_text();
        let id = forge_secrets::crypto::random_token::<18>();
        let answer = self.tag(&id, &text);
        let ip = bucket(ip);

        {
            let mut m = self.inner.lock().ok()?;
            m.pending.retain(|_, p| p.at.elapsed() < TTL);
            m.issued.retain(|_, (_, at)| at.elapsed() < FAILURE_WINDOW);

            if !m.issued.contains_key(&ip) {
                make_room(&mut m.issued, MAX_COUNTERS, |(_, at)| *at);
            }

            let e = m.issued.entry(ip).or_insert((0, Instant::now()));
            if e.0 >= MAX_ISSUED_PER_IP {
                return None;
            }

            e.0 += 1;
            make_room(&mut m.pending, MAX_PENDING, |p| p.at);
            m.pending.insert(id.clone(), Pending { answer, ip, at: Instant::now() });
        }

        Some(Issued { id, png: render_png(&text), expires_in: TTL })
    }

    /// A challenge whose answer is `text`, for integration tests (they run
    /// the server in-process and cannot read the picture). Not reachable
    /// over the API.
    #[doc(hidden)]
    pub fn issue_known(&self, ip: IpAddr, text: &str) -> String {
        let id = forge_secrets::crypto::random_token::<18>();
        let answer = self.tag(&id, text);

        if let Ok(mut m) = self.inner.lock() {
            m.pending.insert(id.clone(), Pending { answer, ip: bucket(ip), at: Instant::now() });
        }

        id
    }

    /// Checks and uses up challenge `id` answered with `answer` from `ip`.
    pub fn verify(&self, id: &str, answer: &str, ip: IpAddr) -> Result<(), Refusal> {
        let Ok(mut m) = self.inner.lock() else { return Err(Refusal::Invalid) };
        let p = m.pending.remove(id).filter(|p| p.at.elapsed() < TTL && p.ip == bucket(ip)).ok_or(Refusal::Invalid)?;
        let mine = self.tag(id, answer);
        if forge_secrets::crypto::constant_time_eq(mine.as_ref(), p.answer.as_ref()) {
            Ok(())
        } else {
            Err(Refusal::Wrong)
        }
    }

    /// A failed sign-in: counts for the name and the address.
    pub fn failed(&self, login: &str, ip: IpAddr) {
        if let Ok(mut m) = self.inner.lock() {
            m.prune();

            for k in [name_key(login), ip_key(ip)] {
                if !m.failures.contains_key(&k) {
                    make_room(&mut m.failures, MAX_COUNTERS, |(_, at)| *at);
                }

                let e = m.failures.entry(k).or_insert((0, Instant::now()));
                if e.1.elapsed() >= FAILURE_WINDOW {
                    e.0 = 0;
                }

                e.0 = e.0.saturating_add(1);
                e.1 = Instant::now();
            }
        }
    }

    /// A successful sign-in clears the name's count. The address keeps its
    /// count, so one success does not hide password spraying.
    pub fn succeeded(&self, login: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.failures.remove(&name_key(login));
        }
    }
}

/// Six characters from [`ALPHABET`], chosen with the system's secure RNG
/// (rejection sampling: no bias).
fn random_text() -> String {
    let n = u8::try_from(ALPHABET.len()).unwrap_or(u8::MAX);
    let limit = u8::MAX - (u8::MAX % n);
    let mut out = String::with_capacity(LEN);

    while out.len() < LEN {
        for b in forge_secrets::crypto::random_bytes::<16>() {
            if b < limit && out.len() < LEN {
                out.push(char::from(ALPHABET[usize::from(b % n)]));
            }
        }
    }

    out
}

// ------------------------------------------------------------- drawing

/// Strokes of each character on a 6 × 10 grid (y grows downwards).
fn glyph(c: u8) -> &'static [&'static [(f32, f32)]] {
    match c {
        b'A' => &[&[(0.0, 10.0), (3.0, 0.0), (6.0, 10.0)], &[(1.2, 6.0), (4.8, 6.0)]],
        b'C' => &[&[(6.0, 1.5), (4.5, 0.0), (1.5, 0.0), (0.0, 2.0), (0.0, 8.0), (1.5, 10.0), (4.5, 10.0), (6.0, 8.5)]],
        b'D' => &[&[(0.0, 0.0), (0.0, 10.0), (3.5, 10.0), (6.0, 7.5), (6.0, 2.5), (3.5, 0.0), (0.0, 0.0)]],
        b'E' => &[&[(6.0, 0.0), (0.0, 0.0), (0.0, 10.0), (6.0, 10.0)], &[(0.0, 5.0), (4.5, 5.0)]],
        b'F' => &[&[(6.0, 0.0), (0.0, 0.0), (0.0, 10.0)], &[(0.0, 5.0), (4.5, 5.0)]],
        b'G' => &[&[
            (6.0, 1.5),
            (4.5, 0.0),
            (1.5, 0.0),
            (0.0, 2.0),
            (0.0, 8.0),
            (1.5, 10.0),
            (4.5, 10.0),
            (6.0, 8.5),
            (6.0, 5.5),
            (3.5, 5.5),
        ]],
        b'H' => &[&[(0.0, 0.0), (0.0, 10.0)], &[(6.0, 0.0), (6.0, 10.0)], &[(0.0, 5.0), (6.0, 5.0)]],
        b'J' => &[&[(6.0, 0.0), (6.0, 8.0), (4.5, 10.0), (1.5, 10.0), (0.0, 8.5)]],
        b'K' => &[&[(0.0, 0.0), (0.0, 10.0)], &[(6.0, 0.0), (0.0, 6.0)], &[(2.0, 4.2), (6.0, 10.0)]],
        b'M' => &[&[(0.0, 10.0), (0.0, 0.0), (3.0, 6.0), (6.0, 0.0), (6.0, 10.0)]],
        b'N' => &[&[(0.0, 10.0), (0.0, 0.0), (6.0, 10.0), (6.0, 0.0)]],
        b'P' => &[&[(0.0, 10.0), (0.0, 0.0), (4.5, 0.0), (6.0, 1.5), (6.0, 3.5), (4.5, 5.0), (0.0, 5.0)]],
        b'Q' => &[
            &[
                (1.5, 0.0),
                (4.5, 0.0),
                (6.0, 2.0),
                (6.0, 8.0),
                (4.5, 10.0),
                (1.5, 10.0),
                (0.0, 8.0),
                (0.0, 2.0),
                (1.5, 0.0),
            ],
            &[(3.5, 7.0), (6.5, 10.5)],
        ],
        b'R' => &[
            &[(0.0, 10.0), (0.0, 0.0), (4.5, 0.0), (6.0, 1.5), (6.0, 3.5), (4.5, 5.0), (0.0, 5.0)],
            &[(3.0, 5.0), (6.0, 10.0)],
        ],
        b'T' => &[&[(0.0, 0.0), (6.0, 0.0)], &[(3.0, 0.0), (3.0, 10.0)]],
        b'U' => &[&[(0.0, 0.0), (0.0, 8.0), (1.5, 10.0), (4.5, 10.0), (6.0, 8.0), (6.0, 0.0)]],
        b'V' => &[&[(0.0, 0.0), (3.0, 10.0), (6.0, 0.0)]],
        b'W' => &[&[(0.0, 0.0), (1.5, 10.0), (3.0, 4.0), (4.5, 10.0), (6.0, 0.0)]],
        b'X' => &[&[(0.0, 0.0), (6.0, 10.0)], &[(6.0, 0.0), (0.0, 10.0)]],
        b'Y' => &[&[(0.0, 0.0), (3.0, 5.0), (6.0, 0.0)], &[(3.0, 5.0), (3.0, 10.0)]],
        b'3' => &[
            &[(0.0, 1.0), (1.5, 0.0), (4.5, 0.0), (6.0, 1.5), (6.0, 3.5), (4.5, 5.0), (2.0, 5.0)],
            &[(4.5, 5.0), (6.0, 6.5), (6.0, 8.5), (4.5, 10.0), (1.5, 10.0), (0.0, 9.0)],
        ],
        b'4' => &[&[(4.5, 10.0), (4.5, 0.0), (0.0, 7.0), (6.0, 7.0)]],
        b'6' => &[&[
            (5.5, 0.5),
            (3.0, 0.0),
            (1.0, 1.5),
            (0.0, 5.0),
            (0.0, 8.5),
            (1.5, 10.0),
            (4.5, 10.0),
            (6.0, 8.5),
            (6.0, 6.5),
            (4.5, 5.0),
            (1.5, 5.0),
            (0.0, 6.5),
        ]],
        b'7' => &[&[(0.0, 0.0), (6.0, 0.0), (2.0, 10.0)]],
        b'9' => &[&[
            (6.0, 3.5),
            (4.5, 5.0),
            (1.5, 5.0),
            (0.0, 3.5),
            (0.0, 1.5),
            (1.5, 0.0),
            (4.5, 0.0),
            (6.0, 1.5),
            (6.0, 5.0),
            (5.0, 8.5),
            (3.0, 10.0),
            (0.5, 9.5),
        ]],
        _ => &[],
    }
}

/// Jitter for the drawing only (the answer comes from the secure RNG):
/// xorshift64*, seeded from the secure RNG per picture.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Self(u64::from_le_bytes(forge_secrets::crypto::random_bytes::<8>()) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;

        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `[lo, hi)`.
    #[allow(clippy::cast_precision_loss)]
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * unit
    }
}

/// A grayscale picture: 255 is white.
struct Canvas {
    px: Vec<f32>,
}

impl Canvas {
    fn new() -> Self {
        Self { px: vec![255.0; WIDTH * HEIGHT] }
    }

    /// Darkens pixels within `width / 2` of the segment, anti-aliased.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, ink: f32) {
        let r = width / 2.0 + 1.0;
        let x0 = (a.0.min(b.0) - r).floor().max(0.0) as usize;
        let x1 = ((a.0.max(b.0) + r).ceil() as usize).min(WIDTH - 1);
        let y0 = (a.1.min(b.1) - r).floor().max(0.0) as usize;
        let y1 = ((a.1.max(b.1) + r).ceil() as usize).min(HEIGHT - 1);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = (dx * dx + dy * dy).max(1e-6);

        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
                let (cx, cy) = (a.0 + t * dx - px, a.1 + t * dy - py);
                let d = (cx * cx + cy * cy).sqrt();
                let cover = (width / 2.0 + 0.5 - d).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let p = &mut self.px[y * WIDTH + x];
                    *p = p.min(255.0 - (255.0 - ink) * cover);
                }
            }
        }
    }

    /// A polyline, cut into short pieces so the warp bends it.
    fn stroke(&mut self, pts: &[(f32, f32)], warp: &impl Fn((f32, f32)) -> (f32, f32), width: f32, ink: f32) {
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let steps = ((len / 2.0).ceil() as usize).max(1);
            #[allow(clippy::cast_precision_loss)]
            let at = |i: usize| {
                let t = i as f32 / steps as f32;
                warp((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t))
            };
            for i in 0..steps {
                self.line(at(i), at(i + 1), width, ink);
            }
        }
    }
}

/// Draws `text` warped, jittered and crossed by noise lines.
#[allow(clippy::cast_precision_loss)]
fn draw(text: &str) -> Canvas {
    let mut rng = Rng::new();
    let mut c = Canvas::new();

    // Light speckle in the background.
    for _ in 0..500 {
        let (x, y) = (rng.range(0.0, WIDTH as f32), rng.range(0.0, HEIGHT as f32));
        let ink = rng.range(150.0, 225.0);
        c.line((x, y), (x + rng.range(-1.5, 1.5), y + rng.range(-1.5, 1.5)), rng.range(0.6, 1.4), ink);
    }

    // One gentle wave over the whole picture.
    let (ax, fx, phx) = (rng.range(2.0, 4.0), rng.range(0.025, 0.045), rng.range(0.0, 6.3));
    let (ay, fy, phy) = (rng.range(1.5, 3.0), rng.range(0.04, 0.07), rng.range(0.0, 6.3));
    let warp = move |(x, y): (f32, f32)| (x + ay * (y * fy + phy).sin(), y + ax * (x * fx + phx).sin());
    let step = (WIDTH as f32 - 24.0) / LEN as f32;

    for (i, ch) in text.bytes().enumerate() {
        let scale = rng.range(3.7, 4.4);
        let angle = rng.range(-0.28, 0.28);
        let shear = rng.range(-0.15, 0.15);
        let cx = 12.0 + step * (i as f32 + 0.5) + rng.range(-3.0, 3.0);
        let cy = HEIGHT as f32 / 2.0 + rng.range(-5.0, 5.0);
        let (sin, cos) = angle.sin_cos();
        let place = |(gx, gy): (f32, f32)| {
            let (x, y) = ((gx - 3.0) * scale, (gy - 5.0) * scale);
            let x = x + shear * y;
            (cx + x * cos - y * sin, cy + x * sin + y * cos)
        };
        let width = rng.range(2.6, 3.4);
        let ink = rng.range(20.0, 70.0);

        for s in glyph(ch) {
            let pts: Vec<(f32, f32)> = s.iter().copied().map(place).collect();
            c.stroke(&pts, &warp, width, ink);
        }
    }

    // Noise curves across the text, thinner than the characters.
    for _ in 0..3 {
        let (y0, amp, f, ph) =
            (rng.range(15.0, HEIGHT as f32 - 15.0), rng.range(4.0, 14.0), rng.range(0.015, 0.04), rng.range(0.0, 6.3));
        let pts: Vec<(f32, f32)> =
            (0..=22).map(|i| (i as f32 * 10.0, y0 + amp * (i as f32 * 10.0 * f + ph).sin())).collect();
        c.stroke(&pts, &|p| p, rng.range(1.2, 1.8), rng.range(60.0, 110.0));
    }

    c
}

/// The picture for `text` as PNG (8-bit grayscale).
#[must_use]
pub fn render_png(text: &str) -> Vec<u8> {
    let c = draw(text);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let gray: Vec<u8> = c.px.iter().map(|v| v.clamp(0.0, 255.0).round() as u8).collect();
    encode_gray_png(WIDTH, HEIGHT, &gray)
}

/// A minimal PNG encoder: one IHDR, one zlib IDAT (filter 0), IEND.
fn encode_gray_png(w: usize, h: usize, gray: &[u8]) -> Vec<u8> {
    use std::io::Write as _;

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }

    let mut raw = Vec::with_capacity((w + 1) * h);

    for row in gray.chunks(w) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&raw);
    let idat = z.finish().unwrap_or_default();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&u32::try_from(w).unwrap_or(0).to_be_bytes());
    ihdr.extend_from_slice(&u32::try_from(h).unwrap_or(0).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 7));
    const OTHER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 8));

    /// The answer of an issued challenge, found by trying every text is not
    /// possible; tests sign their own text with the same key instead.
    fn issue_with(c: &Captchas, text: &str, ip: IpAddr) -> String {
        let id = forge_secrets::crypto::random_token::<18>();
        let answer = c.tag(&id, text);
        c.inner.lock().unwrap().pending.insert(id.clone(), Pending { answer, ip, at: Instant::now() });

        id
    }

    #[test]
    fn the_third_attempt_needs_a_picture_per_name_and_per_address() {
        let c = Captchas::default();
        assert!(!c.required(Some("alice"), IP, 2));
        c.failed("alice", IP);
        assert!(!c.required(Some("alice"), IP, 2), "one failure");
        c.failed("Alice ", IP);
        assert!(c.required(Some("alice"), IP, 2), "two failures for the name");
        assert!(c.required(Some("bob"), IP, 2), "another name from the same address");
        assert!(c.required(Some("alice"), OTHER, 2), "the same name from another address");
        assert!(!c.required(Some("bob"), OTHER, 2));
        assert!(!c.required(Some("alice"), IP, 0), "0 turns it off");
        c.succeeded("alice");
        assert!(!c.required(Some("alice"), OTHER, 2), "a success clears the name");
        assert!(c.required(Some("bob"), IP, 2), "but not the address");
    }

    #[test]
    fn answers_are_single_use_bound_to_the_address_and_case_insensitive() {
        let c = Captchas::default();
        let id = issue_with(&c, "AC3D4E", IP);
        assert_eq!(c.verify(&id, "ac3 d4e", IP), Ok(()));
        assert_eq!(c.verify(&id, "AC3D4E", IP), Err(Refusal::Invalid), "used up");
        let id = issue_with(&c, "AC3D4E", IP);
        assert_eq!(c.verify(&id, "AC3D4F", IP), Err(Refusal::Wrong));
        assert_eq!(c.verify(&id, "AC3D4E", IP), Err(Refusal::Invalid), "a wrong answer uses it up too");
        let id = issue_with(&c, "AC3D4E", IP);
        assert_eq!(c.verify(&id, "AC3D4E", OTHER), Err(Refusal::Invalid), "another address");
        assert_eq!(c.verify("nope", "AC3D4E", IP), Err(Refusal::Invalid));
    }

    #[test]
    fn expired_answers_are_refused() {
        let c = Captchas::default();
        let id = issue_with(&c, "AC3D4E", IP);
        c.inner.lock().unwrap().pending.get_mut(&id).unwrap().at = Instant::now().checked_sub(TTL).unwrap();
        assert_eq!(c.verify(&id, "AC3D4E", IP), Err(Refusal::Invalid));
    }

    #[test]
    fn issuing_is_limited_per_address() {
        let c = Captchas::default();

        for _ in 0..MAX_ISSUED_PER_IP {
            assert!(c.issue(IP).is_some());
        }

        assert!(c.issue(IP).is_none());
        assert!(c.issue(OTHER).is_some());
    }

    /// Review of 9eaffa5: a flood must not lock other users out.
    #[test]
    fn a_full_store_still_issues_to_a_new_client() {
        let c = Captchas::default();

        {
            let mut m = c.inner.lock().unwrap();

            for i in 0..MAX_PENDING {
                let tag = c.tag(&i.to_string(), "AAAAAA");
                m.pending.insert(i.to_string(), Pending { answer: tag, ip: OTHER, at: Instant::now() });
            }
        }

        assert!(c.issue(IP).is_some(), "the oldest picture makes room");
        assert_eq!(c.inner.lock().unwrap().pending.len(), MAX_PENDING);
    }

    #[test]
    fn ipv6_counts_per_64_and_names_have_a_fixed_size_key() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        let c = Captchas::default();

        for _ in 0..MAX_ISSUED_PER_IP / 2 {
            assert!(c.issue(a).is_some());
            assert!(c.issue(b).is_some());
        }

        assert!(c.issue(a).is_none(), "one /64 shares the limit");
        assert!(c.issue(other).is_some(), "another /64 does not");

        // A picture from one address of the /64 is answered from another.
        let id = issue_with(&c, "AC3D4E", bucket(a));
        assert_eq!(c.verify(&id, "AC3D4E", b), Ok(()));
        c.failed("a".repeat(10_000).as_str(), a);
        c.failed("x", b);
        assert!(!c.required(Some("y"), other, 0), "0 turns it off");
        let m = c.inner.lock().unwrap();
        assert!(m.failures.keys().all(|k| k.len() <= 64), "{:?}", m.failures.keys().collect::<Vec<_>>());
        assert_eq!(m.failures.get(&ip_key(a)).map(|v| v.0), Some(2), "both addresses of the /64 count together");
    }

    #[test]
    fn counters_are_capped() {
        let c = Captchas::default();

        for i in 0..(MAX_COUNTERS + 10) {
            c.failed(&format!("user-{i}"), IP);
        }

        assert!(c.inner.lock().unwrap().failures.len() <= MAX_COUNTERS);
        assert!(c.required(Some("anyone"), IP, 2), "the address's own count is the newest, so it stays");
    }

    #[test]
    fn texts_use_only_the_alphabet_and_every_letter_has_strokes() {
        for _ in 0..200 {
            let t = random_text();
            assert_eq!(t.len(), LEN);
            assert!(t.bytes().all(|b| ALPHABET.contains(&b)), "{t}");
        }

        for b in ALPHABET {
            assert!(!glyph(*b).is_empty(), "{}", char::from(*b));
        }

        for bad in b"0O1IL2Z5S8B" {
            assert!(!ALPHABET.contains(bad), "{}", char::from(*bad));
        }
    }

    /// Writes sample pictures to `$CAPTCHA_SAMPLES` to look at them:
    /// `CAPTCHA_SAMPLES=dir cargo test -p forge-server --lib captcha -- --ignored`.
    #[test]
    #[ignore = "writes sample pictures for a human to look at"]
    fn write_samples() {
        let dir = std::path::PathBuf::from(std::env::var("CAPTCHA_SAMPLES").unwrap());

        for i in 0..6 {
            let t = random_text();
            std::fs::write(dir.join(format!("{i}-{t}.png")), render_png(&t)).unwrap();
        }
    }

    #[test]
    fn the_picture_is_a_valid_png_with_ink_in_it() {
        let png = render_png("AC3D4E");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::try_from(WIDTH).unwrap());

        // Decode the IDAT back and check there is dark ink.
        let len = usize::try_from(u32::from_be_bytes(png[33..37].try_into().unwrap())).unwrap();
        assert_eq!(&png[37..41], b"IDAT");
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&png[41..41 + len]), &mut raw).unwrap();
        assert_eq!(raw.len(), (WIDTH + 1) * HEIGHT);
        let dark = raw.iter().filter(|b| **b < 90).count();
        assert!(dark > 800, "{dark} dark pixels");
        assert!(png.ends_with(&[0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82]), "IEND");
    }
}
