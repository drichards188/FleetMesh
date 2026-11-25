//! NMEA 0183 framing for AIS: `!AIVDM` / `!AIVDO` sentences, checksums, NMEA 4.10 tag blocks and
//! multi-sentence assembly.
//!
//! The parser is lenient where real receivers are untidy (leading noise, CR/LF, lower-case
//! checksums, a missing sequence id) and strict where a wrong answer would be worse than none (a
//! bad checksum drops the sentence, a missing or out-of-order fragment drops the message).

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NmeaError {
    Empty,
    /// No `!` or `$` sentence start on the line.
    NoSentence,
    /// A tag block without its closing backslash or checksum.
    BadTagBlock,
    BadTagChecksum,
    MissingChecksum,
    BadChecksum,
    /// A sentence other than VDM or VDO.
    NotAis,
    /// Too few fields, a fragment count out of range, a bad fill-bit count.
    Malformed(&'static str),
    /// A fragment arrived out of order, or a message lost a fragment.
    Fragment,
}

impl std::fmt::Display for NmeaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// NMEA 4.10 tag block fields (IEC 62320-1 spellings are accepted too).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagBlock {
    /// `s:` source or station: the receiver.
    pub source: Option<String>,
    /// `c:` time. Unix seconds in NMEA 4.10; a value above 1e11 is read as milliseconds, which
    /// some receivers send.
    pub time_ms: Option<i64>,
    /// `g:` (or IEC `xGy:`) sentence grouping: (sentence number, total, group id).
    pub group: Option<(u32, u32, u32)>,
    /// `n:` line count.
    pub line: Option<u64>,
    /// `d:` destination.
    pub dest: Option<String>,
    /// `t:` text.
    pub text: Option<String>,
    /// `r:` relative time (unit unspecified by the standard).
    pub relative: Option<i64>,
}

/// One VDM/VDO sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    /// Two-letter talker (`AI`, `AB`, `BS`, ...).
    pub talker: String,
    /// True for VDO (the receiver's own vessel).
    pub own: bool,
    pub total: u8,
    pub number: u8,
    pub seq: Option<u8>,
    pub channel: Option<char>,
    pub payload: String,
    pub fill: u8,
}

/// XOR of every byte, the NMEA checksum.
pub fn checksum(s: &str) -> u8 {
    s.bytes().fold(0u8, |a, b| a ^ b)
}

fn hex2(s: &str) -> Option<u8> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    u8::from_str_radix(&s[..2], 16).ok()
}

/// Split a line into its tag block (if any) and the sentence text from `!` or `$` on.
pub fn split_line(line: &str) -> Result<(Option<TagBlock>, &str), NmeaError> {
    let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if line.is_empty() {
        return Err(NmeaError::Empty);
    }
    let mut tag = None;
    let mut rest = line;
    if let Some(open) = line.find('\\') {
        let start = line.find(['!', '$']);
        // A backslash before the sentence start opens a tag block.
        if start.is_none_or(|s| open < s) {
            let after = &line[open + 1..];
            let close = after.find('\\').ok_or(NmeaError::BadTagBlock)?;
            tag = Some(parse_tag_block(&after[..close])?);
            rest = &after[close + 1..];
        }
    }
    let start = rest.find(['!', '$']).ok_or(NmeaError::NoSentence)?;
    Ok((tag, &rest[start..]))
}

/// Parse the inside of a tag block (between the backslashes), checksum included.
pub fn parse_tag_block(inner: &str) -> Result<TagBlock, NmeaError> {
    let star = inner.rfind('*').ok_or(NmeaError::BadTagBlock)?;
    let body = &inner[..star];
    let want = hex2(&inner[star + 1..]).ok_or(NmeaError::BadTagBlock)?;
    if checksum(body) != want {
        return Err(NmeaError::BadTagChecksum);
    }
    let mut tb = TagBlock::default();
    for field in body.split(',') {
        let Some((key, val)) = field.split_once(':') else {
            continue;
        };
        match key {
            "s" => tb.source = Some(val.to_string()),
            "c" => {
                if let Ok(v) = val.trim().parse::<f64>() {
                    tb.time_ms = Some(if v > 1e11 {
                        v.round() as i64
                    } else {
                        (v * 1000.0).round() as i64
                    });
                }
            }
            "g" => {
                let p: Vec<u32> = val
                    .split('-')
                    .filter_map(|x| x.trim().parse().ok())
                    .collect();
                if p.len() == 3 {
                    tb.group = Some((p[0], p[1], p[2]));
                }
            }
            "n" | "x" => tb.line = val.trim().parse().ok(),
            "d" => tb.dest = Some(val.to_string()),
            "t" | "i" => tb.text = Some(val.to_string()),
            "r" => tb.relative = val.trim().parse().ok(),
            k => {
                // IEC 62320-1 grouping: `1G2:1234`.
                if let Some((a, b)) = k.split_once('G')
                    && let (Ok(n), Ok(t), Ok(id)) = (a.parse(), b.parse(), val.trim().parse())
                {
                    tb.group = Some((n, t, id));
                }
            }
        }
    }
    Ok(tb)
}

/// Parse one `!xxVDM` or `!xxVDO` sentence (from its leading `!` or `$`).
pub fn parse_vdm(s: &str, require_checksum: bool) -> Result<Sentence, NmeaError> {
    let s = s.trim_end();
    let body_end = s.rfind('*');
    let body = match body_end {
        Some(i) => {
            let want = hex2(&s[i + 1..]).ok_or(NmeaError::BadChecksum)?;
            let b = &s[1..i];
            if checksum(b) != want {
                return Err(NmeaError::BadChecksum);
            }
            b
        }
        None if require_checksum => return Err(NmeaError::MissingChecksum),
        None => &s[1..],
    };
    let f: Vec<&str> = body.split(',').collect();
    let head = f[0];
    if head.len() < 5 {
        return Err(NmeaError::NotAis);
    }
    let (talker, kind) = head.split_at(head.len() - 3);
    let own = match kind {
        "VDM" => false,
        "VDO" => true,
        _ => return Err(NmeaError::NotAis),
    };
    if f.len() < 7 {
        return Err(NmeaError::Malformed("too few fields"));
    }
    let total: u8 = f[1]
        .trim()
        .parse()
        .map_err(|_| NmeaError::Malformed("fragment count"))?;
    let number: u8 = f[2]
        .trim()
        .parse()
        .map_err(|_| NmeaError::Malformed("fragment number"))?;
    if total == 0 || number == 0 || number > total || total > 9 {
        return Err(NmeaError::Malformed("fragment numbering"));
    }
    let seq = if f[3].trim().is_empty() {
        None
    } else {
        Some(
            f[3].trim()
                .parse()
                .map_err(|_| NmeaError::Malformed("sequence id"))?,
        )
    };
    let channel = f[4].trim().chars().next();
    let payload = f[5].trim().to_string();
    let fill: u8 = if f[6].trim().is_empty() {
        0
    } else {
        f[6].trim()
            .parse()
            .map_err(|_| NmeaError::Malformed("fill bits"))?
    };
    if fill > 5 {
        return Err(NmeaError::Malformed("fill bits"));
    }
    if payload.is_empty() {
        return Err(NmeaError::Malformed("empty payload"));
    }
    Ok(Sentence {
        talker: talker.to_string(),
        own,
        total,
        number,
        seq,
        channel,
        payload,
        fill,
    })
}

/// A complete AIS payload, from one sentence or several.
#[derive(Debug, Clone, PartialEq)]
pub struct Assembled {
    pub payload: String,
    pub fill: u8,
    pub channel: Option<char>,
    pub own: bool,
    /// The receiver: the tag block's `s:`, else the caller's default.
    pub rx: String,
    /// The tag block's `c:` time from the first fragment that had one.
    pub time_ms: Option<i64>,
    pub fragments: u8,
}

#[derive(Debug)]
struct Partial {
    total: u8,
    next: u8,
    payload: String,
    time_ms: Option<i64>,
    started_ms: i64,
    channel: Option<char>,
    own: bool,
}

/// Reassembles multi-sentence messages. Fragments are keyed by receiver, channel and sequence
/// id (or the tag block's group id), so interleaved messages on two channels or from two
/// receivers on one stream stay apart.
#[derive(Debug, Default)]
pub struct Assembler {
    partial: HashMap<(String, Option<char>, u32), Partial>,
    /// A partial message older than this (by receipt time) is abandoned.
    pub max_age_ms: i64,
    pub dropped: u64,
}

impl Assembler {
    pub fn new(max_age_ms: i64) -> Self {
        Assembler {
            partial: HashMap::new(),
            max_age_ms,
            dropped: 0,
        }
    }

    /// Offer one sentence. Returns the payload when a message is complete.
    pub fn push(
        &mut self,
        tag: Option<&TagBlock>,
        s: Sentence,
        default_rx: &str,
        now_ms: i64,
    ) -> Result<Option<Assembled>, NmeaError> {
        let rx = tag
            .and_then(|t| t.source.clone())
            .unwrap_or_else(|| default_rx.to_string());
        let time_ms = tag.and_then(|t| t.time_ms);
        if s.total == 1 {
            return Ok(Some(Assembled {
                payload: s.payload,
                fill: s.fill,
                channel: s.channel,
                own: s.own,
                rx,
                time_ms,
                fragments: 1,
            }));
        }
        // Old partials are abandoned (a lost last fragment).
        if self.partial.len() > 16 {
            let max_age = self.max_age_ms;
            let before = self.partial.len();
            self.partial.retain(|_, p| now_ms - p.started_ms <= max_age);
            self.dropped += (before - self.partial.len()) as u64;
        }
        let gid = match (tag.and_then(|t| t.group), s.seq) {
            (Some((_, _, g)), _) => 1000 + g,
            (None, Some(q)) => q as u32,
            (None, None) => 999,
        };
        let key = (rx.clone(), s.channel, gid);
        if s.number == 1 {
            if self.partial.remove(&key).is_some() {
                self.dropped += 1;
            }
            self.partial.insert(
                key,
                Partial {
                    total: s.total,
                    next: 2,
                    payload: s.payload,
                    time_ms,
                    started_ms: now_ms,
                    channel: s.channel,
                    own: s.own,
                },
            );
            return Ok(None);
        }
        let Some(p) = self.partial.get_mut(&key) else {
            self.dropped += 1;
            return Err(NmeaError::Fragment);
        };
        if p.total != s.total || p.next != s.number || now_ms - p.started_ms > self.max_age_ms {
            self.partial.remove(&key);
            self.dropped += 1;
            return Err(NmeaError::Fragment);
        }
        p.payload.push_str(&s.payload);
        p.next += 1;
        if p.time_ms.is_none() {
            p.time_ms = time_ms;
        }
        if s.number == s.total {
            let Some(p) = self.partial.remove(&key) else {
                return Ok(None);
            };
            return Ok(Some(Assembled {
                payload: p.payload,
                fill: s.fill,
                channel: p.channel,
                own: p.own,
                rx,
                time_ms: p.time_ms,
                fragments: s.total,
            }));
        }
        Ok(None)
    }

    pub fn pending(&self) -> usize {
        self.partial.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_and_fields() {
        let s = parse_vdm("!AIVDM,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0*4A", true).expect("valid");
        assert_eq!(s.talker, "AI");
        assert!(!s.own);
        assert_eq!(
            (s.total, s.number, s.seq, s.channel, s.fill),
            (1, 1, None, Some('A'), 0)
        );
        assert_eq!(
            parse_vdm("!AIVDM,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0*4B", true),
            Err(NmeaError::BadChecksum)
        );
        // Lower-case checksum hex is accepted.
        assert!(parse_vdm("!AIVDM,1,1,,B,25Cjtd0Oj;Jp7ilG7=UkKBoB0<06,0*60", true).is_ok());
        assert_eq!(
            parse_vdm("!GPRMC,1,2,3,4,5,6,7", false),
            Err(NmeaError::NotAis)
        );
        assert_eq!(
            parse_vdm("!AIVDM,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0", true),
            Err(NmeaError::MissingChecksum)
        );
    }
}
