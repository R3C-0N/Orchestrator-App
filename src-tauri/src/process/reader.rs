//! Lecteur de lignes de stdout et stderr de `claude.exe`.
//!
//! Le découpage est fait sur les **octets** (`\n`), et le décodage UTF-8 sur la
//! ligne complète : un caractère multioctet coupé entre deux lectures est donc
//! recollé avant décodage. Règles :
//!
//! - fin de ligne `\n` ou `\r\n` (le `\r` final est retiré) ;
//! - dernière ligne sans `\n` : rendue à la fin du flux ;
//! - UTF-8 invalide : remplacé par U+FFFD (décodage avec perte), jamais d'échec ;
//! - plafond de taille ([`MAX_LINE_BYTES`]) : une ligne plus longue n'est pas
//!   gardée en mémoire ; on lit jusqu'à son `\n` et on rend [`RawLine::TooLong`]
//!   avec sa taille et son début, puis la lecture continue normalement ;
//! - lignes vides : rendues telles quelles (`Line("")`), l'appelant les écarte.
//!
//! Plafond de 64 Mio : le plan ne fixe pas de valeur (spec 4.8 : « sans limite
//! fixe », plan 1.9 et J6 : « plafond de taille »). La plus longue ligne des
//! fixtures fait 51 ko (`t11`, réponse à `initialize`) ; un `tool_result` peut
//! porter des images en base64 (quelques Mo chacune) ou une longue sortie
//! d'outil. 64 Mio laisse deux ordres de grandeur de marge sur ces cas tout en
//! bornant la mémoire si le flux s'emballe (une ligne sans fin).

use std::collections::VecDeque;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

/// Taille maximale d'une ligne, fin de ligne exclue.
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;
/// Octets du début d'une ligne trop longue gardés pour le journal.
pub const TOO_LONG_HEAD: usize = 500;
/// Taille des lectures sur le tuyau.
const CHUNK: usize = 64 * 1024;

/// Une ligne lue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawLine {
    /// Ligne décodée, sans `\n` ni `\r` final.
    Line(String),
    /// Ligne au-delà du plafond : taille en octets et début (avec perte).
    TooLong { len: usize, head: String },
}

/// Découpe synchrone d'un flux d'octets en lignes, alimentée par morceaux.
#[derive(Debug)]
pub struct LineSplitter {
    buf: Vec<u8>,
    max: usize,
    /// Ligne en cours au-delà du plafond : (taille vue, début).
    overflow: Option<(usize, Vec<u8>)>,
}

impl Default for LineSplitter {
    fn default() -> Self {
        Self::new(MAX_LINE_BYTES)
    }
}

impl LineSplitter {
    pub fn new(max: usize) -> Self {
        LineSplitter {
            buf: Vec::new(),
            max,
            overflow: None,
        }
    }

    /// Consomme un morceau et ajoute à `out` les lignes qu'il termine.
    pub fn feed(&mut self, mut chunk: &[u8], out: &mut VecDeque<RawLine>) {
        while let Some(i) = chunk.iter().position(|&b| b == b'\n') {
            self.append(&chunk[..i]);
            out.push_back(self.take());
            chunk = &chunk[i + 1..];
        }
        self.append(chunk);
    }

    /// Fin du flux : la dernière ligne, si elle n'avait pas de `\n`.
    pub fn finish(&mut self) -> Option<RawLine> {
        if self.buf.is_empty() && self.overflow.is_none() {
            None
        } else {
            Some(self.take())
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        if let Some((len, _)) = &mut self.overflow {
            *len += bytes.len();
            return;
        }
        if self.buf.len() + bytes.len() > self.max {
            let mut head = std::mem::take(&mut self.buf);
            let len = head.len() + bytes.len();
            if head.len() < TOO_LONG_HEAD {
                let more = (TOO_LONG_HEAD - head.len()).min(bytes.len());
                head.extend_from_slice(&bytes[..more]);
            }
            head.truncate(TOO_LONG_HEAD);
            head.shrink_to_fit();
            self.overflow = Some((len, head));
        } else {
            self.buf.extend_from_slice(bytes);
        }
    }

    fn take(&mut self) -> RawLine {
        if let Some((len, head)) = self.overflow.take() {
            return RawLine::TooLong {
                len,
                head: String::from_utf8_lossy(&head).into_owned(),
            };
        }
        let mut b = std::mem::take(&mut self.buf);
        if b.last() == Some(&b'\r') {
            b.pop();
        }
        RawLine::Line(match String::from_utf8(b) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    }
}

/// Découpe un tampon complet (fixtures, outils).
pub fn split_all(bytes: &[u8], max: usize) -> Vec<RawLine> {
    let mut s = LineSplitter::new(max);
    let mut out = VecDeque::new();
    s.feed(bytes, &mut out);
    out.extend(s.finish());
    out.into()
}

/// Lecteur asynchrone de lignes sur un tuyau.
pub struct LineReader<R> {
    inner: R,
    splitter: LineSplitter,
    ready: VecDeque<RawLine>,
    chunk: Vec<u8>,
    eof: bool,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_max(inner, MAX_LINE_BYTES)
    }

    pub fn with_max(inner: R, max: usize) -> Self {
        LineReader {
            inner,
            splitter: LineSplitter::new(max),
            ready: VecDeque::new(),
            chunk: vec![0; CHUNK],
            eof: false,
        }
    }

    /// Ligne suivante ; `Ok(None)` à la fin du flux.
    pub async fn next_line(&mut self) -> io::Result<Option<RawLine>> {
        loop {
            if let Some(l) = self.ready.pop_front() {
                return Ok(Some(l));
            }
            if self.eof {
                return Ok(None);
            }
            let n = self.inner.read(&mut self.chunk).await?;
            if n == 0 {
                self.eof = true;
                self.ready.extend(self.splitter.finish());
            } else {
                self.splitter.feed(&self.chunk[..n], &mut self.ready);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(parts: &[&[u8]], max: usize) -> Vec<RawLine> {
        let mut s = LineSplitter::new(max);
        let mut out = VecDeque::new();
        for p in parts {
            s.feed(p, &mut out);
        }
        out.extend(s.finish());
        out.into()
    }

    fn l(s: &str) -> RawLine {
        RawLine::Line(s.into())
    }

    #[test]
    fn crlf_last_line_and_empty_lines() {
        assert_eq!(
            lines(&[b"a\r\n\r\nb\n\nc"], 100),
            vec![l("a"), l(""), l("b"), l(""), l("c")]
        );
        assert_eq!(lines(&[b"a\n"], 100), vec![l("a")]);
        assert_eq!(lines(&[b""], 100), vec![]);
        // `\r` coupé de son `\n` par une frontière de lecture.
        assert_eq!(lines(&[b"a\r", b"\nb"], 100), vec![l("a"), l("b")]);
    }

    #[test]
    fn utf8_split_across_chunks() {
        let s = "é€😀".as_bytes();
        for cut in 1..s.len() {
            let (a, b) = s.split_at(cut);
            assert_eq!(lines(&[a, b, b"\n"], 100), vec![l("é€😀")], "coupe {cut}");
        }
    }

    #[test]
    fn too_long_line_is_skipped_and_reading_continues() {
        let long = [b'x'; 30];
        let out = lines(&[b"ok\n", &long[..10], &long[10..], b"\nsuite\n"], 20);
        assert_eq!(out[0], l("ok"));
        assert!(matches!(&out[1], RawLine::TooLong { len: 30, head } if head.len() == 30));
        assert_eq!(out[2], l("suite"));
        // Exactement au plafond : gardée.
        assert_eq!(lines(&[&long[..20], b"\n"], 20), vec![l(&"x".repeat(20))]);
    }
}
