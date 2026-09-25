//! Agent contracts for reading and rearranging a Git review.

use super::{SideEffect, StrictJsonSchema, ToolDefinition};
use crate::ai::scope::RequiredScope;
use crate::ai::types::FileScope;
use serde_json::json;

pub const READ_DIFF: &str = "read_diff";
pub const SHOW_CUSTOM_DIFF: &str = "show_custom_diff";

pub fn read_diff_definition() -> ToolDefinition {
    ToolDefinition {
        name: READ_DIFF.into(),
        description: "Read Ovim's Git review using the same comparison as <leader>gd, including its configured pullbase. Finish edits first, then call without snapshot_id to freeze the current comparison and get block IDs for added and removed lines. To read the next page, pass snapshot_id and next_cursor as cursor; continue until next_cursor is null. Read all pages before assigning changes. After further edits, start a fresh snapshot. The comparison base follows Ovim's pullbase setting; this tool does not override it.".into(),
        required_scope: RequiredScope { file_scope: FileScope::Project, shell: false, network: false },
        side_effect: SideEffect::Read,
        custom_input_schema: Some(StrictJsonSchema::new(json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "snapshot_id": { "type": "string", "minLength": 1, "description": "Snapshot ID returned by an earlier read_diff call. Omit to capture the current comparison." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Maximum changed lines returned (default 100)." },
                "cursor": { "type": "string", "minLength": 1, "maxLength": 512, "description": "Opaque next_cursor returned by the previous page. Requires snapshot_id." }
            }
        })).expect("valid read_diff schema")),
        parameters: vec![],
    }
}

pub fn show_custom_diff_definition() -> ToolDefinition {
    ToolDefinition {
        name: SHOW_CUSTOM_DIFF.into(),
        description: "Open a replayable review immediately without waiting for dismissal. First read every page of read_diff's frozen comparison. The pairings array defines ordered sections: provide old (removed block) and new (added block) for a replacement, old alone for a deletion, or new alone for an addition. At least one side is required. Use zero-based offset and positive count to split a block into disjoint semantic slices; pair lengths may differ. Label deletions explicitly (for example, 'Remove duplicate permission check') instead of pairing them with unrelated nearby additions. For copied or extracted code, assign each changed line once, then use related_to to reference an existing removed or added range: for example an old/new pair for the primary move, followed by a new-only section labelled 'Additional copy' with related_to pointing to the original old range. related_to is an explanatory source reference, not another pairing, and cannot overlap this section's own lines; it does not consume, duplicate, hide, or prove equivalence of code. Several sections may reference the same source. Labels describe intent but cannot change addition/deletion kinds. Unassigned changes, metadata, and binary changes remain visible automatically. Overlapping ownership, invalid ranges, and wrong-side references are rejected. GUI Guided view and the terminal show these sections; Files keeps canonical file order. All content comes from the frozen snapshot, never agent-authored code.".into(),
        required_scope: RequiredScope { file_scope: FileScope::Project, shell: false, network: false },
        side_effect: SideEffect::Navigation,
        custom_input_schema: Some(StrictJsonSchema::new(json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "snapshot_id": { "type": "string", "minLength": 1 },
                "title": { "type": "string", "minLength": 1, "maxLength": 200 },
                "pairings": {
                    "type": "array", "maxItems": 1000,
                    "items": {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "label": { "type": "string", "maxLength": 200 },
                            "old": { "$ref": "#/$defs/reference" },
                            "new": { "$ref": "#/$defs/reference" },
                            "related_to": { "$ref": "#/$defs/reference", "description": "Explanatory reference to a removed or added range outside this section. Does not own its lines or imply equality." }
                        },
                        "anyOf": [{"required": ["old"]}, {"required": ["new"]}]
                    }
                }
            },
            "required": ["snapshot_id", "title", "pairings"],
            "$defs": {
                "reference": {
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "block_id": { "type": "string", "minLength": 1 },
                        "offset": { "type": "integer", "minimum": 0 },
                        "count": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["block_id"]
                }
            }
        })).expect("valid show_custom_diff schema")),
        parameters: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn section_schema_and_runtime_shape_agree() {
        let definition = show_custom_diff_definition();
        let schema = definition.custom_input_schema.unwrap();
        let reference = json!({"block_id": "removed_1", "offset": 0, "count": 1});
        for section in [
            json!({"old": reference}),
            json!({"new": reference}),
            json!({"old": reference, "new": reference}),
            json!({"new": reference, "related_to": reference}),
        ] {
            schema
                .validate_instance(
                    &json!({"snapshot_id":"s", "title":"Review", "pairings":[section]}),
                )
                .unwrap();
            let parsed: crate::native_diff::DiffPairing =
                serde_json::from_value(section.clone()).unwrap();
            assert_eq!(
                parsed.related_to.is_some(),
                section.get("related_to").is_some()
            );
        }
        for section in [
            json!({}),
            json!({"label":"Deleted"}),
            json!({"related_to":reference}),
            json!({"old":null}),
            json!({"old":reference,"kind":"context"}),
            json!({"old":reference,"text":"invented"}),
            json!({"old":reference,"related_to":{"block_id":"x","count":0}}),
        ] {
            assert!(schema
                .validate_instance(
                    &json!({"snapshot_id":"s", "title":"Review", "pairings":[section]})
                )
                .is_err());
        }
    }
}
