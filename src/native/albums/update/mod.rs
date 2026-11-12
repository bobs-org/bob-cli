//! Guarded album schema update internals.
//!
//! `protocol` parses the JSON envelope once; `ops` applies ordered
//! operations with complete-record impact accounting, shared Rust
//! validation, typed refusals, and semantic inverses; `spans` preserves
//! schema bytes; `plan` composes the read-only update; `run` executes the
//! guarded `bob albums update` transaction and its JSON/human output.
//!
//! Wire examples handed to schema-spans:
//! - `{"schema_version":1,"expect":"sha256:…","ops":[]}` is a valid
//!   no-op envelope; `-i -` reads stdin and omitted `-i` means the same.
//! - `{"op":"album.set","set":{"label":"Eateries"},"unset":["plural"]}`
//! - `{"op":"field.add","label":"Status","kind":"choices","position":0,
//!    "choices":[{"label":"To try"},{"label":"Liked","emoji":"💚"}]}`
//! - `{"op":"field.set","field":"status",
//!    "set":{"required":true},"unset":[]}`
//! - `{"op":"field.move","field":"status","position":0}`
//! - `{"op":"field.remove","field":"status"}`
//! - `{"op":"choice.add","field":"status","label":"Regular",
//!    "emoji":"🔁","position":2}`
//! - `{"op":"choice.set","field":"status","value":"regular",
//!    "set":{"label":"Regular","emoji":"🔁"}}` (empty emoji removes icon)
//! - `{"op":"choice.move","field":"status","value":"regular",
//!    "position":0}`
//! - `{"op":"choice.remove","field":"status","value":"regular"}`
//! - Undo-only restoration metadata: `field.add` accepts
//!   `restore_key` and `restore_values`, `choice.add` accepts
//!   `restore_value`. Each only creates an absent key, never renames,
//!   and inverse field removals restore required/default/bounds (and
//!   absent label/explicit suggest-false) through a follow-up `field.set`.
//!   `restore_values` preserves stored choice keys in declaration order
//!   when they differ from their labels.
//!   Inverse ops may also carry `restore: {"spans":[{"key_path":
//!   "record.fields.status.label","expected":"...","prior":"..."}]}`.
//!   `expected` and `prior` are exact schema-entry bytes or `null` for an
//!   absent key; paths are resolved under `record` and no offsets are trusted.
//!
//! Interfaces handed to schema-spans:
//! - `protocol::parse_envelope(&str) -> Result<Envelope, ProtocolError>`
//! - `ops::apply_ops(&[Op], &ApplyInputs) -> Result<OpOutcome, OpError>`
//! - `ops::ApplyInputs { before_raw, before_type, before_plural, stem,
//!   folder, index_refs, other_fields, today }`
//! - `ops::OpOutcome { after_raw, after_type, after_plural, per_op,
//!   derived_keys, consequence, inverse_ops, group_before/after }`
//! - `ops::IndexRefs { index_by, index_facets }` and
//!   `ops::OtherAlbumField { path, key, storage, list }` for the
//!   planner's registry overlay checks.

pub(crate) mod ops;
pub(crate) mod plan;
pub(crate) mod protocol;
pub(crate) mod run;
pub(crate) mod spans;
