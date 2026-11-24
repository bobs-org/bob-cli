//! Expand one setting gesture into the existing reversible span operations.

use serde_json::Value as JsonValue;
use serde_yaml::Value;

use super::{error, settings, validate};
use crate::native::props::{
    document::{PropertyKind, PropsDocument},
    emit::emit_value,
    protocol::Op,
    InspectContext, PropsError,
};

pub(crate) fn expand(
    context: &InspectContext,
    document: &PropsDocument,
    op: &Op,
    index: usize,
) -> Result<Vec<Op>, PropsError> {
    let (id, value) = match op {
        Op::SettingSet { setting, value } => (setting.as_str(), Some(value)),
        Op::SettingClear { setting } => (setting.as_str(), None),
        _ => return Ok(vec![op.clone()]),
    };
    let setting =
        settings::by_id(context.roles.note_kind, id).ok_or_else(|| {
            error(
                "prop_setting_disabled",
                id,
                index,
                format!("setting {id:?} is unavailable on this note kind"),
                "Use a setting defined for this area or project.",
            )
        })?;
    if value.is_none() {
        for key in setting.keys {
            validate::removal(context, key, index)?;
        }
        return Ok(clear(document, setting.keys));
    }
    let value = value.expect("set value");
    if id == "ready_cap"
        && !(value.as_u64().is_some()
            || value
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("off")))
    {
        return Err(validate::bad(id, index, "Use an integer 1–999 or off."));
    }
    if id == "task_refresh" && value.as_u64().is_none() {
        return Err(validate::bad(id, index, "Use an integer 1–365 days."));
    }

    if id == "clock" {
        return clock(context, document, value, index);
    }
    let yaml = serde_yaml::to_value(value)
        .map_err(|_| validate::bad(id, index, "Use a JSON setting value."))?;
    let (canonical, _) =
        validate::value(context, &document.source, id, &yaml, index)?;
    let mut ops = vec![set(id, canonical)];
    if id == "task_type" {
        ops.extend(clear(document, &["theme"]));
    }
    Ok(ops)
}

fn clear(document: &PropsDocument, keys: &[&str]) -> Vec<Op> {
    keys.iter()
        .filter(|key| document.entries.iter().any(|entry| entry.key == **key))
        .map(|key| Op::Unset {
            key: (*key).to_owned(),
        })
        .collect()
}

fn set(key: &str, value: Value) -> Op {
    Op::Set {
        key: key.to_owned(),
        value: serde_json::to_value(value).expect("JSON setting"),
        kind: None,
        position: None,
    }
}

fn clock(
    context: &InspectContext,
    document: &PropsDocument,
    value: &JsonValue,
    index: usize,
) -> Result<Vec<Op>, PropsError> {
    let hint = "Use {\"mode\":\"inherit\"}, {\"mode\":\"on\"}, {\"mode\":\"off\"}, or {\"mode\":\"types\",\"types\":[\"MEET\"]}.";
    let object = value
        .as_object()
        .ok_or_else(|| validate::bad("clock", index, hint))?;
    let mode = object
        .get("mode")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| validate::bad("clock", index, hint))?;
    if object
        .keys()
        .any(|key| key != "mode" && !(mode == "types" && key == "types"))
    {
        return Err(validate::bad("clock", index, hint));
    }
    match mode {
        "inherit" => Ok(clear(document, &["offclock", "offclock_types"])),
        "on" | "off" => {
            let mut ops = vec![set("offclock", Value::Bool(mode == "off"))];
            ops.extend(clear(document, &["offclock_types"]));
            Ok(ops)
        }
        "types" => {
            let types = object
                .get("types")
                .filter(|value| value.is_array())
                .ok_or_else(|| validate::bad("clock", index, hint))?;
            let yaml = serde_yaml::to_value(types).expect("JSON list");
            let (canonical, _) = validate::value(
                context,
                &document.source,
                "offclock_types",
                &yaml,
                index,
            )?;
            let yaml =
                emit_value(&canonical, PropertyKind::List, None, false, "\n");
            // set_yaml uses block form even if the old list used flow form.
            let mut ops = vec![Op::SetYaml {
                key: "offclock_types".to_owned(),
                yaml,
                position: None,
            }];
            ops.extend(clear(document, &["offclock"]));
            Ok(ops)
        }
        _ => Err(validate::bad("clock", index, hint)),
    }
}
