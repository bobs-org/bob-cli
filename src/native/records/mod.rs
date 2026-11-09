//! Record Note engine: schema, per-folder catalog, renderer, and planner.
//!
//! Shared Record Note schema, catalog, planner, renderer, and public CLI.
#![allow(dead_code)]

pub(crate) mod catalog;
pub(crate) mod check;
pub(crate) mod cli;
pub(crate) mod init;
pub(crate) mod plan;
pub(crate) mod render;
pub(crate) mod schema;
pub(crate) mod stem;

#[allow(unused_imports)]
pub(crate) use catalog::{
    FolderIndex, IndexedField, IndexedFieldValue, IndexedRecord, NearValueHint,
    RecordCatalog, StemLookup, ValueSuggestion,
};
#[allow(unused_imports)]
pub(crate) use plan::{
    display_scalar, display_value, plan_create, plan_fields, plan_update,
    resolve_update_target, PlanUpdateRequest, PlannedRecordProperty,
    PlannedUpdateProperty, RecordCreatePlan, RecordCreateRequest,
    RecordFieldPlan, RecordPlanError, RecordPlanErrorCode,
    RecordPropertySource, RecordPropertyState, RecordStemSource,
    RecordUpdatePlan, RecordUpdateTargetError, RecordUpdateTargetErrorCode,
    ResolvedUpdateTarget,
};
#[allow(unused_imports)]
pub(crate) use render::{
    AppliedRecordUpdate, RecordNoteInput, RecordRenderRequest,
    RecordUpdateError, RecordUpdateErrorCode, RecordUpdateRequest,
    RenderedRecord,
};
#[allow(unused_imports)]
pub(crate) use schema::{
    FieldInputState, FieldKind, RecordChoice, RecordDate, RecordDiagnostic,
    RecordDiagnosticCode, RecordField, RecordFieldInput, RecordScalar,
    RecordType, RecordValue, RecordsConfig, ValidatedFieldInput,
};
