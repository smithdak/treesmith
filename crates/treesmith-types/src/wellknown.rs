//! Well-known platform GUID vocabulary (DESIGN.md §12).
//!
//! Interpretation of spec invariant I6: I6 quarantines *serialization
//! dialect* knowledge in `treesmith-format`; well-known platform GUIDs are
//! the domain vocabulary the whole kernel resolves against, so they live
//! here under neutral constant names.
//!
//! Values are canonical lowercase-hyphenated and validated at compile time
//! by the `uuid!` macro.

use crate::Guid;

/// Template definition item.
pub const TEMPLATE_TEMPLATE: Guid = Guid(uuid::uuid!("ab86861a-6030-46c5-b394-e8f99e8b87db"));

/// Template section.
pub const TEMPLATE_SECTION: Guid = Guid(uuid::uuid!("e269fbb5-3750-427a-9149-7aa950b49301"));

/// Template field.
pub const TEMPLATE_FIELD: Guid = Guid(uuid::uuid!("455a3e98-a627-4b40-8035-e683a0331ac7"));

/// Template folder.
pub const TEMPLATE_FOLDER: Guid = Guid(uuid::uuid!("0437fee2-44c9-46a6-abe9-28858d9fee8c"));

/// Standard template.
pub const STANDARD_TEMPLATE: Guid = Guid(uuid::uuid!("1930bbeb-7805-471a-a3be-4858ac7cf696"));

/// Common folder.
pub const FOLDER: Guid = Guid(uuid::uuid!("a87a00b1-e6db-45ab-8b54-636fec3b5523"));

/// `__Base template` field.
pub const BASE_TEMPLATE_FIELD: Guid = Guid(uuid::uuid!("12c33f3f-86c5-43a5-aeb4-5598cec45116"));

/// Template's standard-values pointer field.
pub const STANDARD_VALUES_FIELD: Guid = Guid(uuid::uuid!("f7d48a55-2158-4f02-9356-756654404f73"));

/// `Type` field on a template field.
pub const FIELD_TYPE_FIELD: Guid = Guid(uuid::uuid!("ab162cc0-dc80-4abf-8871-998ee5d7ba32"));

/// `Shared` checkbox on a template field.
pub const FIELD_SHARED_FIELD: Guid = Guid(uuid::uuid!("be351a73-fcb0-4213-93fa-c302d8ab4f51"));

/// `Unversioned` checkbox on a template field.
pub const FIELD_UNVERSIONED_FIELD: Guid = Guid(uuid::uuid!("39847666-389d-409b-95bd-f2016f11eed5"));

/// `__Renderings` field (shared layout).
pub const LAYOUT_FIELD: Guid = Guid(uuid::uuid!("f1a1fe9e-a60c-4ddb-a3a0-bb5b29fe732e"));

/// `__Final Renderings` field (versioned layout delta).
pub const FINAL_LAYOUT_FIELD: Guid = Guid(uuid::uuid!("04bf00db-f5fb-41f7-8ab7-22408372a981"));

/// `__Display name` field.
pub const DISPLAY_NAME_FIELD: Guid = Guid(uuid::uuid!("b5e02ad9-d56f-4c41-a065-a133db87bdeb"));

/// `__Sortorder` field.
pub const SORTORDER_FIELD: Guid = Guid(uuid::uuid!("ba3f86a2-4a1c-4d78-b63d-91c2779c1b5e"));

/// `__Created` field.
pub const CREATED_FIELD: Guid = Guid(uuid::uuid!("25bed78c-4957-4165-998a-ca1b52f67497"));

/// `__Created by` field.
pub const CREATED_BY_FIELD: Guid = Guid(uuid::uuid!("5dd74568-4d4b-44c1-b513-0af5f4cda34f"));

/// View rendering template.
pub const VIEW_RENDERING: Guid = Guid(uuid::uuid!("99f8905d-4a87-4eb8-9f8b-a9bebfb3add6"));

/// Controller rendering template.
pub const CONTROLLER_RENDERING: Guid = Guid(uuid::uuid!("2a3e91a0-7987-44b5-ab34-35c2d9de83b9"));

/// Layout template.
pub const LAYOUT: Guid = Guid(uuid::uuid!("3a45a723-64ee-4919-9d41-02fd40fd1466"));

/// Placeholder settings template.
pub const PLACEHOLDER_SETTINGS: Guid = Guid(uuid::uuid!("5c547d4e-7111-4995-95b0-6b561751bf2e"));

/// Default device.
pub const DEFAULT_DEVICE: Guid = Guid(uuid::uuid!("fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3"));

/// `Path` field on a layout item.
///
/// VERIFY-P0: training-vintage id, to be checked against real client repos
/// in the P0 census. Code must not depend on this id alone — name/hint
/// fallback is the primary resolution path for `Path`/`Controller`
/// (DESIGN.md §6.4, §12).
pub const LAYOUT_PATH_FIELD: Guid = Guid(uuid::uuid!("07aa88dc-3b4b-4e85-91f2-a4cc5261c6d4"));

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The full DESIGN.md §12 table, restated so edits to either side get
    /// caught: (name, constant, canonical lowercase-hyphenated guid).
    const ALL: &[(&str, Guid, &str)] = &[
        (
            "TEMPLATE_TEMPLATE",
            TEMPLATE_TEMPLATE,
            "ab86861a-6030-46c5-b394-e8f99e8b87db",
        ),
        (
            "TEMPLATE_SECTION",
            TEMPLATE_SECTION,
            "e269fbb5-3750-427a-9149-7aa950b49301",
        ),
        (
            "TEMPLATE_FIELD",
            TEMPLATE_FIELD,
            "455a3e98-a627-4b40-8035-e683a0331ac7",
        ),
        (
            "TEMPLATE_FOLDER",
            TEMPLATE_FOLDER,
            "0437fee2-44c9-46a6-abe9-28858d9fee8c",
        ),
        (
            "STANDARD_TEMPLATE",
            STANDARD_TEMPLATE,
            "1930bbeb-7805-471a-a3be-4858ac7cf696",
        ),
        ("FOLDER", FOLDER, "a87a00b1-e6db-45ab-8b54-636fec3b5523"),
        (
            "BASE_TEMPLATE_FIELD",
            BASE_TEMPLATE_FIELD,
            "12c33f3f-86c5-43a5-aeb4-5598cec45116",
        ),
        (
            "STANDARD_VALUES_FIELD",
            STANDARD_VALUES_FIELD,
            "f7d48a55-2158-4f02-9356-756654404f73",
        ),
        (
            "FIELD_TYPE_FIELD",
            FIELD_TYPE_FIELD,
            "ab162cc0-dc80-4abf-8871-998ee5d7ba32",
        ),
        (
            "FIELD_SHARED_FIELD",
            FIELD_SHARED_FIELD,
            "be351a73-fcb0-4213-93fa-c302d8ab4f51",
        ),
        (
            "FIELD_UNVERSIONED_FIELD",
            FIELD_UNVERSIONED_FIELD,
            "39847666-389d-409b-95bd-f2016f11eed5",
        ),
        (
            "LAYOUT_FIELD",
            LAYOUT_FIELD,
            "f1a1fe9e-a60c-4ddb-a3a0-bb5b29fe732e",
        ),
        (
            "FINAL_LAYOUT_FIELD",
            FINAL_LAYOUT_FIELD,
            "04bf00db-f5fb-41f7-8ab7-22408372a981",
        ),
        (
            "DISPLAY_NAME_FIELD",
            DISPLAY_NAME_FIELD,
            "b5e02ad9-d56f-4c41-a065-a133db87bdeb",
        ),
        (
            "SORTORDER_FIELD",
            SORTORDER_FIELD,
            "ba3f86a2-4a1c-4d78-b63d-91c2779c1b5e",
        ),
        (
            "CREATED_FIELD",
            CREATED_FIELD,
            "25bed78c-4957-4165-998a-ca1b52f67497",
        ),
        (
            "CREATED_BY_FIELD",
            CREATED_BY_FIELD,
            "5dd74568-4d4b-44c1-b513-0af5f4cda34f",
        ),
        (
            "VIEW_RENDERING",
            VIEW_RENDERING,
            "99f8905d-4a87-4eb8-9f8b-a9bebfb3add6",
        ),
        (
            "CONTROLLER_RENDERING",
            CONTROLLER_RENDERING,
            "2a3e91a0-7987-44b5-ab34-35c2d9de83b9",
        ),
        ("LAYOUT", LAYOUT, "3a45a723-64ee-4919-9d41-02fd40fd1466"),
        (
            "PLACEHOLDER_SETTINGS",
            PLACEHOLDER_SETTINGS,
            "5c547d4e-7111-4995-95b0-6b561751bf2e",
        ),
        (
            "DEFAULT_DEVICE",
            DEFAULT_DEVICE,
            "fe5d7fdf-89c0-4d99-9aa3-b5fbd009c9f3",
        ),
        (
            "LAYOUT_PATH_FIELD",
            LAYOUT_PATH_FIELD,
            "07aa88dc-3b4b-4e85-91f2-a4cc5261c6d4",
        ),
    ];

    #[test]
    fn table_is_complete() {
        // DESIGN.md §12 lists exactly 23 well-known GUIDs.
        assert_eq!(ALL.len(), 23);
    }

    #[test]
    fn constants_match_design_table() {
        for (name, guid, canonical) in ALL {
            assert_eq!(
                guid.rainbow(),
                *canonical,
                "{name} does not match DESIGN.md §12"
            );
            assert_eq!(*guid, Guid::parse(canonical).unwrap(), "{name} reparse");
        }
    }

    #[test]
    fn constants_are_distinct() {
        let unique: HashSet<Guid> = ALL.iter().map(|(_, g, _)| *g).collect();
        assert_eq!(unique.len(), ALL.len(), "duplicate well-known GUID");
    }
}
