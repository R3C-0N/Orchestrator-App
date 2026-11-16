//! Écrivain du stdin de `claude.exe` : une ligne JSON par message (spec 4.2).
//!
//! Les lignes passent par un canal et sont écrites dans l'ordre par une seule
//! tâche : deux envois simultanés ne peuvent pas s'entrelacer. Chaque ligne et
//! son `\n` partent en un seul `write_all`, suivi d'un `flush`. Fermer le canal
//! (lâcher tous les émetteurs) ferme stdin, ce qui termine `claude.exe`.

use std::io;

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Écrit une ligne suivie de `\n`, puis vide le tampon. Une ligne contenant
/// déjà un saut de ligne est refusée : elle casserait le découpage du CLI.
pub async fn write_line<W: AsyncWrite + Unpin>(w: &mut W, line: &str) -> io::Result<()> {
    if line.contains('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ligne sortante contenant un saut de ligne",
        ));
    }
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    w.write_all(&buf).await?;
    w.flush().await
}

/// Lance la tâche d'écriture. Elle s'arrête à la fermeture du canal ou à la
/// première erreur d'écriture (stdin fermé par le CLI), et ferme alors `w`.
///
/// Une ligne contenant un saut de ligne est rejetée seule (journalisée), sans
/// fermer stdin : c'est un bogue de l'appelant, pas une panne du tuyau. Une
/// erreur d'écriture est journalisée avant l'arrêt de la tâche.
///
/// Journal : `eprintln!` faute de façade de journalisation dans le crate ;
/// à brancher sur le journal de J6 (`logging.rs`).
pub fn spawn_writer<W>(w: W) -> (mpsc::UnboundedSender<String>, JoinHandle<io::Result<()>>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let mut w = w;
        while let Some(line) = rx.recv().await {
            if line.contains('\n') {
                eprintln!(
                    "[writer] ligne rejetée (saut de ligne interne, {} octets) : {}",
                    line.len(),
                    line.chars().take(200).collect::<String>()
                );
                continue;
            }
            if let Err(e) = write_line(&mut w, &line).await {
                eprintln!("[writer] écriture sur stdin impossible, arrêt de l'écrivain : {e}");
                return Err(e);
            }
        }
        // Lâcher `w` ferme le tuyau : le CLI voit la fin de stdin. Pas de
        // `shutdown()` : chaque ligne est déjà vidée par `write_line`.
        drop(w);
        Ok(())
    });
    (tx, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn lines_in_order_then_close() {
        let (a, mut b) = tokio::io::duplex(1024);
        let (tx, task) = spawn_writer(a);
        tx.send("{\"n\":1}".into()).unwrap();
        tx.send("{\"n\":2}".into()).unwrap();
        drop(tx);
        task.await.unwrap().unwrap();
        let mut s = String::new();
        b.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "{\"n\":1}\n{\"n\":2}\n");
    }

    #[tokio::test]
    async fn a_bad_line_is_skipped_without_closing_stdin() {
        let (a, mut b) = tokio::io::duplex(1024);
        let (tx, task) = spawn_writer(a);
        tx.send("{\"n\":1}".into()).unwrap();
        tx.send(
            "{\"x\":\"a
b\"}"
                .into(),
        )
        .unwrap();
        tx.send("{\"n\":2}".into()).unwrap();
        drop(tx);
        task.await.unwrap().unwrap();
        let mut s = String::new();
        b.read_to_string(&mut s).await.unwrap();
        assert_eq!(
            s,
            "{\"n\":1}
{\"n\":2}
"
        );
    }

    #[tokio::test]
    async fn write_error_ends_the_task_with_the_error() {
        let (a, b) = tokio::io::duplex(8);
        drop(b); // lecteur fermé : l'écriture échoue
        let (tx, task) = spawn_writer(a);
        tx.send("{\"n\":1}".into()).unwrap();
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn embedded_newline_is_refused() {
        let mut out = Vec::new();
        let e = write_line(&mut out, "a\nb").await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(out.is_empty());
    }
}
