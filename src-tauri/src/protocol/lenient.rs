//! Désérialiseurs tolérants pour les champs du flux.
//!
//! Le flux de `claude.exe` n'est pas versionné et gagne des champs sans
//! prévenir (décision 12) : un champ absent, nul ou d'un type inattendu ne doit
//! jamais faire échouer la lecture d'un message entier. Chaque champ typé des
//! structures de `inbound` et `control` passe donc par l'une de ces fonctions,
//! avec `#[serde(default, deserialize_with = "…")]` :
//!
//! | Fonction | Valeur reçue | Résultat |
//! | - | - | - |
//! | [`opt`] | du bon type | `Some(v)` |
//! | [`opt`] | absente, `null` ou d'un autre type | `None` |
//! | [`raw`] | présente (même `null`) | `Some(valeur brute)` |
//! | [`obj`] | présente (même `null` ou d'un autre type) | `Some(T)`, champs par défaut si illisible |
//! | [`strings`] | tableau | ses chaînes (les autres éléments sont écartés) |
//! | [`vec_or_default`] | tableau | un `T` par élément (`T::default()` si illisible) |
//!
//! [`obj`] reproduit la sémantique de `v.get("champ")` du réducteur de J0 : la
//! présence du champ compte, même quand son contenu est inutilisable.

use std::fmt;
use std::marker::PhantomData;

use serde::de::value::MapAccessDeserializer;
use serde::de::{DeserializeOwned, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Champ facultatif : `None` s'il est absent, nul ou d'un type inattendu.
pub fn opt<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = Value::deserialize(d)?;
    Ok(T::deserialize(v).ok())
}

/// Valeur JSON brute, conservée telle quelle dès que le champ est présent.
pub fn raw<'de, D>(d: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(d).map(Some)
}

/// Sous-objet : présent = `Some`, même si son contenu est illisible (il prend
/// alors ses valeurs par défaut, toutes tolérantes elles aussi).
///
/// Lu sans passer par une `Value` intermédiaire (c'est le champ le plus gros
/// des messages : `message` d'un `assistant`). `T` doit être entièrement
/// tolérant : lu depuis un objet, il ne peut alors échouer que sur une erreur
/// de syntaxe JSON.
pub fn obj<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    d.deserialize_any(ObjVisitor(PhantomData)).map(Some)
}

struct ObjVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de> + Default> Visitor<'de> for ObjVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("un objet JSON")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::deserialize(MapAccessDeserializer::new(map))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<T, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(T::default())
    }

    fn visit_bool<E>(self, _: bool) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_i64<E>(self, _: i64) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_u64<E>(self, _: u64) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_f64<E>(self, _: f64) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_str<E>(self, _: &str) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_unit<E>(self) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_none<E>(self) -> Result<T, E> {
        Ok(T::default())
    }
}

/// Tableau de chaînes ; les éléments d'un autre type sont écartés.
pub fn strings<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a
            .into_iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// Tableau d'objets : un élément illisible donne `T::default()`, il n'est pas
/// écarté (les positions restent celles du flux).
pub fn vec_or_default<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a
            .into_iter()
            .map(|x| T::deserialize(x).unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    })
}

/// Désérialise une structure entièrement tolérante depuis une valeur ; une
/// valeur qui n'est pas un objet donne la structure par défaut.
pub fn from_value<T: DeserializeOwned + Default>(v: &Value) -> T {
    T::deserialize(v).unwrap_or_default()
}

/// `v[k]` en chaîne, s'il y en a une.
pub fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Inner {
        #[serde(deserialize_with = "opt")]
        n: Option<u64>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct S {
        #[serde(deserialize_with = "opt")]
        a: Option<String>,
        #[serde(deserialize_with = "raw")]
        r: Option<Value>,
        #[serde(deserialize_with = "obj")]
        o: Option<Inner>,
        #[serde(deserialize_with = "strings")]
        l: Vec<String>,
        #[serde(deserialize_with = "vec_or_default")]
        v: Vec<Inner>,
    }

    #[test]
    fn wrong_types_never_fail() {
        let s: S =
            from_value(&json!({"a": 5, "r": null, "o": 7, "l": [1, "x"], "v": [2, {"n": 3}]}));
        assert_eq!(s.a, None);
        assert_eq!(s.r, Some(Value::Null), "présent mais nul");
        assert_eq!(s.o.map(|o| o.n), Some(None), "présent mais illisible");
        assert_eq!(s.l, vec!["x".to_string()]);
        assert_eq!(
            s.v.iter().map(|i| i.n).collect::<Vec<_>>(),
            vec![None, Some(3)]
        );
    }

    #[test]
    fn absent_fields_are_none() {
        let s: S = from_value(&json!({"autre": true}));
        assert!(s.a.is_none() && s.r.is_none() && s.o.is_none());
        assert!(s.l.is_empty() && s.v.is_empty());
        let s: S = from_value(&json!(42));
        assert!(s.a.is_none());
    }
}
