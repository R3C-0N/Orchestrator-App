//! Tests ciblés du lecteur de lignes (`process::reader`) et du décodage
//! (`protocol::inbound`) : ligne de 10 Mo, UTF-8 coupé à la frontière du
//! tampon, CRLF, ligne vide, JSON invalide, ligne au-delà du plafond.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use orchestrator_lib::process::reader::{LineReader, RawLine};
use orchestrator_lib::protocol::inbound::Inbound;
use tokio::io::{AsyncRead, ReadBuf};

/// Lecteur qui rend les octets par morceaux de tailles imposées (le dernier
/// morceau se répète), pour placer les frontières de lecture où l'on veut.
struct Chunked {
    data: Vec<u8>,
    pos: usize,
    sizes: Vec<usize>,
    i: usize,
}

impl Chunked {
    fn new(data: Vec<u8>, sizes: &[usize]) -> Self {
        Chunked {
            data,
            pos: 0,
            sizes: sizes.to_vec(),
            i: 0,
        }
    }
}

impl AsyncRead for Chunked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let size = self.sizes[self.i.min(self.sizes.len() - 1)];
        self.i += 1;
        let n = size.min(buf.remaining()).min(self.data.len() - self.pos);
        let start = self.pos;
        buf.put_slice(&self.data[start..start + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

async fn read_all<R: AsyncRead + Unpin>(mut r: LineReader<R>) -> Vec<RawLine> {
    let mut out = Vec::new();
    while let Some(l) = r.next_line().await.unwrap() {
        out.push(l);
    }
    out
}

fn line(s: &str) -> RawLine {
    RawLine::Line(s.to_string())
}

#[tokio::test]
async fn ten_megabyte_line_is_read_whole() {
    let text = "é".repeat(5 * 1024 * 1024); // 10 Mio d'octets
    let json = format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
    );
    let mut data = json.clone().into_bytes();
    data.extend_from_slice(b"\n{\"type\":\"keep_alive\"}\n");
    let lines = read_all(LineReader::new(Chunked::new(data, &[7, 65_536]))).await;
    assert_eq!(lines.len(), 2);
    let RawLine::Line(l) = &lines[0] else {
        panic!("ligne de 10 Mo refusée")
    };
    assert!(l.len() > 10 * 1024 * 1024);
    let Inbound::Assistant(a) = Inbound::from_line(l).unwrap() else {
        panic!()
    };
    let content = a.message.unwrap().content.unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(lines[1], line(r#"{"type":"keep_alive"}"#));
}

#[tokio::test]
async fn utf8_cut_at_the_buffer_boundary() {
    // Le tampon de lecture fait 64 Kio : on place chaque coupe possible d'un
    // caractère de 4 octets juste sur cette frontière.
    let emoji = "😀";
    for cut in 1..emoji.len() {
        let mut data = vec![b'a'; 65_536 - cut];
        data.extend_from_slice(emoji.as_bytes());
        data.extend_from_slice("€\n".as_bytes());
        let lines = read_all(LineReader::new(Chunked::new(data, &[65_536]))).await;
        let RawLine::Line(l) = &lines[0] else {
            panic!()
        };
        assert!(l.ends_with("😀€"), "coupe à {cut} octet(s)");
        assert!(!l.contains('\u{FFFD}'), "aucun caractère de remplacement");
    }
    // Coupe en morceaux d'un octet.
    let data = "{\"t\":\"àé😀\"}\n".as_bytes().to_vec();
    let lines = read_all(LineReader::new(Chunked::new(data, &[1]))).await;
    assert_eq!(lines, vec![line("{\"t\":\"àé😀\"}")]);
}

#[tokio::test]
async fn crlf_empty_lines_and_last_line_without_newline() {
    let data = b"{\"a\":1}\r\n\r\n\n{\"b\":2}\r\n{\"c\":3}".to_vec();
    let lines = read_all(LineReader::new(Chunked::new(data, &[3]))).await;
    assert_eq!(
        lines,
        vec![
            line("{\"a\":1}"),
            line(""),
            line(""),
            line("{\"b\":2}"),
            line("{\"c\":3}"),
        ]
    );
}

#[tokio::test]
async fn invalid_utf8_is_replaced_not_fatal() {
    let data = b"{\"a\":\"\xff\"}\n{\"b\":1}\n".to_vec();
    let lines = read_all(LineReader::new(Chunked::new(data, &[4]))).await;
    assert_eq!(lines[0], line("{\"a\":\"\u{FFFD}\"}"));
    assert_eq!(lines[1], line("{\"b\":1}"));
}

#[tokio::test]
async fn invalid_json_fails_alone_and_reading_continues() {
    let data =
        b"{\"type\":\"result\"\n{pas du json}\n{\"type\":\"result\",\"subtype\":\"success\"}\n"
            .to_vec();
    let lines = read_all(LineReader::new(Chunked::new(data, &[5]))).await;
    let decoded: Vec<_> = lines
        .iter()
        .map(|l| match l {
            RawLine::Line(l) => Inbound::from_line(l).is_ok(),
            RawLine::TooLong { .. } => panic!(),
        })
        .collect();
    assert_eq!(decoded, vec![false, false, true]);
}

#[tokio::test]
async fn line_over_the_cap_is_skipped_then_reading_continues() {
    let mut data = vec![b'x'; 3000];
    data.extend_from_slice(b"\n{\"ok\":1}\n");
    let r = LineReader::with_max(Chunked::new(data, &[100]), 1000);
    let lines = read_all(r).await;
    let RawLine::TooLong { len, head } = &lines[0] else {
        panic!("{:?}", lines[0])
    };
    assert_eq!(*len, 3000);
    assert_eq!(head.len(), 500);
    assert_eq!(lines[1], line("{\"ok\":1}"));
}
