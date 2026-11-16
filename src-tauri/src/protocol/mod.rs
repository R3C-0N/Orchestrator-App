//! Protocole stream-json de `claude.exe` (spec section 4).
//!
//! - [`inbound`] : messages lus sur stdout (enums tolérants) ;
//! - [`control`] : requêtes et réponses de contrôle, dans les deux sens ;
//! - [`outbound`] : lignes écrites sur stdin ;
//! - [`fixture`] : lecture des captures `fixtures/stream-json/*.jsonl` ;
//! - [`lenient`] : désérialiseurs tolérants communs.

pub mod control;
pub mod fixture;
pub mod inbound;
pub mod lenient;
pub mod outbound;
