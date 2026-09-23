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
        description: "Open a replayable review immediately and return without waiting for dismissal. First call read_diff to obtain Ovim's configured pullbase comparison and block references. Pair related removals and additions, including across files; unassigned changes remain visible automatically. Each old reference must name a removed block and each new reference an added block. Optional offset and count select a zero-based slice of a block; disjoint slices can be paired separately. The review uses the frozen snapshot even if the workspace changes afterward, preserving every canonical added and removed line from that snapshot.".into(),
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
                            "new": { "$ref": "#/$defs/reference" }
                        },
                        "required": ["old", "new"]
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
