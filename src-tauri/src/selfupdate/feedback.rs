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
//! retour).
//!
//! Étape 0 : constantes figées ; le formatage est écrit à l'étape A.

/// Taille maximale d'un retour d'erreur envoyé à la session.
pub const MAX_FEEDBACK_BYTES: usize = 8 * 1024;

/// Lignes de sortie gardées par commande en échec (fin de la sortie).
pub const MAX_LINES_PER_STEP: usize = 80;
