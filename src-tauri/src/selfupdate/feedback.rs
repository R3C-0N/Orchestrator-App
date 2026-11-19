//! Jalon J16 (onglet d'amélioration) : texte renvoyé à la session dédiée
//! quand un contrôle ou un test corrigible échoue. Propriétaire : agent A.
//!
//! Règles (docs/plan-prototype-1.md, J16) : en-tête
//! `[Contrôle automatique — tentative n/3]`, étape, raison, extrait de sortie
//! (au plus [`MAX_LINES_PER_STEP`] lignes par commande, au plus
//! [`MAX_FEEDBACK_BYTES`] octets en tout, troncature signalée), séquences
//! ANSI et caractères de contrôle retirés, chemins du worktree et du
//! worktree de tests remplacés par `.`, dossier de l'app par `<app>`,
//! dossier personnel par `~` ; jamais l'environnement, `config.json` ni la
//! raison d'une violation de sécurité (celle-ci arrête le cycle sans
//! retour : le pipeline n'appelle jamais [`format_feedback`] pour elle).

use std::path::Path;

/// Taille maximale d'un retour d'erreur envoyé à la session.
pub const MAX_FEEDBACK_BYTES: usize = 8 * 1024;

/// Lignes de sortie gardées par commande en échec (fin de la sortie).
pub const MAX_LINES_PER_STEP: usize = 80;

/// Lignes gardées dans le `detail` court du statut (bandeau, onglet).
pub const DETAIL_LINES: usize = 20;

/// Remplacements de chemins locaux dans le texte renvoyé à la session.
#[derive(Debug, Clone, Default)]
pub struct Redactions {
    /// (chemin, remplacement), du plus long au plus court : un chemin inclus
    /// dans un autre (worktree sous le dossier de l'app) est remplacé
    /// d'abord par le plus précis.
    pairs: Vec<(String, String)>,
}

impl Redactions {
    pub fn new() -> Self {
        Redactions::default()
    }

    /// Remplace `path` par `by`, sous toutes ses formes : séparateurs `\`,
    /// `/`, `\\` (JSON) ou mélangés, casse ASCII ignorée. Chemin vide ou
    /// trop court (racine) : ignoré.
    pub fn add(mut self, path: &Path, by: &str) -> Self {
        let s = path.to_string_lossy();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let s = s.trim_end_matches(['/', '\\']).replace('\\', "/");
        if s.len() < 4 || self.pairs.iter().any(|(p, _)| p.eq_ignore_ascii_case(&s)) {
            return self;
        }
        self.pairs.push((s, by.to_string()));
        self.pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        self
    }

    /// `text` avec les chemins remplacés, puis tout dossier d'utilisateur
    /// restant (`C:\Users\<nom>`, forme courte 8.3 comprise) remplacé par `~`.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (p, by) in &self.pairs {
            out = replace_path(&out, p, by);
        }
        mask_user_dirs(&out)
    }
}

const BACKSLASH: u8 = 0x5c;

/// Caractères qui terminent un nom d'utilisateur dans un chemin.
const NAME_END: &[u8] = b" \t\n\"':;,()[]<>|";

fn is_sep(b: u8) -> bool {
    b == b'/' || b == BACKSLASH
}

/// Longueur de l'occurrence de `needle` (séparateurs `/`) au début de `h` :
/// un séparateur de `needle` accepte `/`, `\` ou `\\`, le reste est comparé
/// sans casse ASCII.
fn path_match(h: &[u8], needle: &[u8]) -> Option<usize> {
    let mut i = 0;
    for &n in needle {
        if n == b'/' {
            if !h.get(i).copied().is_some_and(is_sep) {
                return None;
            }
            i += 1;
            if h[i - 1] == BACKSLASH && h.get(i) == Some(&BACKSLASH) {
                i += 1;
            }
        } else {
            if !h.get(i).is_some_and(|c| c.eq_ignore_ascii_case(&n)) {
                return None;
            }
            i += 1;
        }
    }
    Some(i)
}

/// Remplace toutes les occurrences du chemin `needle` (voir [`path_match`])
/// par `by`.
fn replace_path(hay: &str, needle: &str, by: &str) -> String {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    let mut out = String::with_capacity(hay.len());
    let mut last = 0;
    let mut i = 0;
    while i < h.len() {
        match path_match(&h[i..], n) {
            Some(len) if hay.is_char_boundary(i) && hay.is_char_boundary(i + len) => {
                out.push_str(&hay[last..i]);
                out.push_str(by);
                i += len;
                last = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&hay[last..]);
    out
}

/// Remplace par `~` tout `<lecteur>:\Users\<nom>` (ou `/Users/<nom>`,
/// `/c/Users/<nom>`, séparateurs quelconques), y compris un nom court 8.3
/// (`MMEDAR~1`) : le dossier personnel n'apparaît sous aucune forme.
fn mask_user_dirs(text: &str) -> String {
    let h = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < h.len() {
        // `<sep>users<sep>`
        let hit = is_sep(h[i])
            && h.len() >= i + 7
            && h[i + 1..i + 6].eq_ignore_ascii_case(b"users")
            && is_sep(h[i + 6]);
        if !hit {
            i += 1;
            continue;
        }
        let mut start = i;
        // Séparateur doublé avant `users` (JSON).
        if start > 0 && h[start] == BACKSLASH && h[start - 1] == BACKSLASH {
            start -= 1;
        }
        // Lecteur `C:` ou `/c` juste avant ; sinon `users` doit être à la
        // racine du chemin (`src/users/x` n'est pas un dossier d'utilisateur).
        let pathish = |b: u8| b.is_ascii_alphanumeric() || b"._-~$".contains(&b) || is_sep(b);
        let drive = start >= 2 && h[start - 1] == b':' && h[start - 2].is_ascii_alphabetic();
        let msys_drive = start >= 2
            && h[start - 1].is_ascii_alphabetic()
            && h[start - 2] == b'/'
            && (start == 2 || !pathish(h[start - 3]));
        if drive || msys_drive {
            start -= 2;
        } else if start > 0 && pathish(h[start - 1]) {
            i += 1;
            continue;
        }
        let mut j = i + 7;
        if h[j - 1] == BACKSLASH && h.get(j) == Some(&BACKSLASH) {
            j += 1;
        }
        let name_start = j;
        while j < h.len() && !is_sep(h[j]) && !NAME_END.contains(&h[j]) {
            j += 1;
        }
        if j == name_start || !text.is_char_boundary(start) || !text.is_char_boundary(j) {
            i += 1;
            continue;
        }
        out.push_str(&text[last..start]);
        out.push('~');
        last = j;
        i = j;
    }
    out.push_str(&text[last..]);
    out
}

/// Texte sans séquences ANSI (CSI, OSC, autres échappements) ni caractères
/// de contrôle ; `\r\n` et `\r` deviennent `\n`, la tabulation est gardée.
/// Saute une séquence CSI (paramètres puis octet final 0x40–0x7E) ; non
/// terminée, elle s'arrête à la fin de la ligne (saut de ligne gardé).
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    for d in chars.by_ref() {
        if d == '\n' {
            out.push(d);
            return;
        }
        if ('\u{40}'..='\u{7e}').contains(&d) {
            return;
        }
    }
}

pub fn strip_control(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // CSI : paramètres puis un octet final 0x40–0x7E.
                // Séquence non terminée : coupée jusqu'à la fin de la ligne
                // seulement (le saut de ligne est gardé).
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars, &mut out);
                }
                // OSC : jusqu'à BEL ou ESC \ (ou la fin de la ligne).
                Some(']') => {
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d == '\n' {
                            out.push(d);
                            break;
                        }
                        if d == '\u{7}' {
                            break;
                        }
                        if d == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Autre échappement : un caractère.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // CSI sur 8 bits.
            '\u{9b}' => skip_csi(&mut chars, &mut out),
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Ligne qui pourrait porter un secret ou l'environnement (variables
/// d'authentification, `config.json` de l'app) : masquée.
fn is_sensitive(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "anthropic_",
        "config.json",
        "api_key",
        "apikey",
        "token=",
        "token:",
        "secret",
        "password",
        "passwd",
        "authorization:",
    ]
    .iter()
    .any(|m| l.contains(m))
}

/// Texte nettoyé : contrôle retiré, chemins remplacés, lignes sensibles
/// masquées, blancs de fin de ligne retirés.
pub fn clean(text: &str, redact: &Redactions) -> String {
    let text = redact.apply(&strip_control(text));
    text.lines()
        .map(|l| {
            if is_sensitive(l) {
                "[ligne masquée]"
            } else {
                l.trim_end()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `detail` court du statut : première ligne, puis les dernières lignes
/// (au plus [`DETAIL_LINES`] en tout).
pub fn short_detail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= DETAIL_LINES {
        return text.trim_end().to_string();
    }
    let mut v = vec![lines[0], "…"];
    v.extend(&lines[lines.len() - (DETAIL_LINES - 2)..]);
    v.join("\n")
}

/// Message renvoyé à la session après l'échec corrigible `text` à l'étape
/// `step` (libellé lisible : « contrôles », « tests »…), tentative
/// `attempt` sur `max`. La première ligne de `text` est la raison, le reste
/// l'extrait de sortie (fin gardée : [`MAX_LINES_PER_STEP`] lignes,
/// [`MAX_FEEDBACK_BYTES`] octets pour tout le message).
pub fn format_feedback(
    attempt: u32,
    max: u32,
    step: &str,
    text: &str,
    redact: &Redactions,
) -> String {
    let text = clean(text, redact);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let reason = lines
        .next()
        .unwrap_or("échec sans message")
        .trim()
        .trim_end_matches([':', ' '])
        .to_string();
    let reason = cut_bytes(&reason, 1024);
    let mut excerpt: Vec<&str> = lines.collect();
    let mut dropped = 0usize;
    if excerpt.len() > MAX_LINES_PER_STEP {
        dropped = excerpt.len() - MAX_LINES_PER_STEP;
        excerpt.drain(..dropped);
    }
    let head = format!(
        "[Contrôle automatique — tentative {attempt}/{max}]\nÉtape : {step}\nRaison : {reason}\n"
    );
    let tail = "\nCorrige ce problème dans le worktree, puis commite les fichiers touchés \
                (sans `git add -A`). Les contrôles et les tests repartent à la fin de ton \
                tour. Si la correction est impossible dans le périmètre, explique pourquoi \
                sans commiter."
        .to_string();
    if excerpt.is_empty() {
        return format!("{head}{tail}");
    }
    // Lignes retirées en tête tant que le message dépasse la limite.
    let budget = MAX_FEEDBACK_BYTES;
    loop {
        let note = if dropped > 0 {
            format!("[… {dropped} ligne(s) précédente(s) tronquée(s)]\n")
        } else {
            String::new()
        };
        let body = excerpt.join("\n");
        let msg = format!("{head}Extrait de la sortie :\n```\n{note}{body}\n```\n{tail}");
        if msg.len() <= budget {
            return msg;
        }
        if excerpt.len() > 1 {
            excerpt.remove(0);
            dropped += 1;
            continue;
        }
        // Une seule ligne trop longue : coupée.
        let room = budget.saturating_sub(msg.len() - body.len() + 64);
        let one = cut_bytes(excerpt[0], room);
        return format!(
            "{head}Extrait de la sortie :\n```\n[… sortie tronquée à {} Ko]\n{one}\n```\n{tail}",
            MAX_FEEDBACK_BYTES / 1024
        );
    }
}

/// Fin de `s` sur au plus `max` octets (limite de caractère respectée).
fn cut_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unterminated_sequences_cut_to_end_of_line_only() {
        assert_eq!(strip_control("a\u{1b}[31\nb\nc"), "a\nb\nc");
        assert_eq!(strip_control("a\u{1b}]0;titre\nb"), "a\nb");
        assert_eq!(strip_control("a\u{9b}12\nb"), "a\nb");
        assert_eq!(strip_control("fin\u{1b}["), "fin");
    }

    #[test]
    fn mixed_separators_and_json_escapes() {
        let r = Redactions::new().add(
            Path::new(r"C:\Users\moi\AppData\Local\orchestrator\work\auto\x"),
            ".",
        );
        for t in [
            r"C:\Users\moi/AppData\Local/orchestrator\work/auto\x/src/a.ts",
            r"c:/USERS/MOI/appdata/local/orchestrator/work/auto/x/src/a.ts",
            r"C:\\Users\\moi\\AppData\\Local\\orchestrator\\work\\auto\\x/src/a.ts",
        ] {
            assert_eq!(r.apply(t), "./src/a.ts", "{t}");
        }
    }

    #[test]
    fn any_user_dir_is_masked_including_short_names() {
        let r = Redactions::new();
        assert_eq!(
            r.apply(r"at C:\Users\MMEDAR~1\AppData\x.js"),
            r"at ~\AppData\x.js"
        );
        assert_eq!(
            r.apply("voir C:/Users/autre/p et /c/Users/moi/q"),
            "voir ~/p et ~/q"
        );
        assert_eq!(r.apply(r#"{"p":"C:\\Users\\moi\\a"}"#), r#"{"p":"~\\a"}"#);
        assert_eq!(r.apply("/Users/moi/x (mac)"), "~/x (mac)");
        // Pas un dossier d'utilisateur.
        assert_eq!(r.apply("src/users/list.ts"), "src/users/list.ts");
        assert_eq!(r.apply("UserService"), "UserService");
    }

    #[test]
    fn strips_ansi_and_controls() {
        let s = "\u{1b}[31mrouge\u{1b}[0m\r\nok\u{7}\u{1b}]0;titre\u{7}fin\rbis\u{0}\tz";
        assert_eq!(strip_control(s), "rouge\nokfin\nbis\tz");
    }

    #[test]
    fn redacts_paths_any_separator_and_case() {
        let r = Redactions::new()
            .add(
                Path::new(r"C:\Users\moi\AppData\Local\orchestrator\work\auto\x"),
                ".",
            )
            .add(
                Path::new(r"C:\Users\moi\AppData\Local\orchestrator"),
                "<app>",
            )
            .add(Path::new(r"C:\Users\moi"), "~");
        let t = r"at C:/users/MOI/AppData/Local/orchestrator/work/auto/x/src/a.ts:3
C:\Users\moi\AppData\Local\orchestrator\ui\h.tmp
C:\\Users\\moi\\.cargo";
        assert_eq!(r.apply(t), "at ./src/a.ts:3\n<app>\\ui\\h.tmp\n~\\\\.cargo");
    }

    #[test]
    fn feedback_is_bounded_and_flags_truncation() {
        let mut text = String::from("npm test a échoué :\n");
        for i in 0..500 {
            text.push_str(&format!("ligne {i} {}\n", "x".repeat(60)));
        }
        let m = format_feedback(1, 3, "tests", &text, &Redactions::new());
        assert!(m.len() <= MAX_FEEDBACK_BYTES, "{}", m.len());
        assert!(m.starts_with("[Contrôle automatique — tentative 1/3]\nÉtape : tests\n"));
        assert!(m.contains("Raison : npm test a échoué\n"));
        assert!(m.contains("tronquée"));
        assert!(m.contains("ligne 499"));
        assert!(!m.contains("ligne 419 "));
        let body = m.split("```").nth(1).unwrap();
        assert!(body.lines().filter(|l| l.starts_with("ligne")).count() <= MAX_LINES_PER_STEP);
        // Une seule ligne géante.
        let m = format_feedback(
            2,
            3,
            "tests",
            &format!("r\n{}", "y".repeat(20_000)),
            &Redactions::new(),
        );
        assert!(m.len() <= MAX_FEEDBACK_BYTES && m.contains("tronquée à 8 Ko"));
    }

    #[test]
    fn sensitive_lines_are_masked() {
        let m = format_feedback(
            1,
            3,
            "tests",
            "échec\nANTHROPIC_API_KEY=sk-123\nvoir <app>/config.json\nautre",
            &Redactions::new(),
        );
        assert!(!m.contains("sk-123") && !m.contains("config.json"), "{m}");
        assert!(m.contains("autre"));
    }

    #[test]
    fn short_detail_keeps_first_and_last_lines() {
        let t: String = (0..50).map(|i| format!("l{i}\n")).collect();
        let d = short_detail(&t);
        assert_eq!(d.lines().count(), DETAIL_LINES);
        assert!(d.starts_with("l0\n…\n") && d.ends_with("l49"));
        assert_eq!(short_detail("a\nb"), "a\nb");
    }
}
