infer_opaque_hidden_type =
    opaque type's hidden type cannot be another opaque type from the same scope
    .label = one of the two opaque types used here has to be outside its defining scope
    .opaque_type = opaque type whose hidden type is being assigned
    .hidden_type = opaque type being used as hidden type

# M27 Phase 5b Stage F: the rustc Diagnostic derive's `#[note(slug)]`
# resolves slugs against `crate::fluent_generated` directly, not through
# the subdiagnostic-attribute lookup that upstream uses. Mirror the two
# subdiag attributes above as crate-prefixed top-level slugs so
# `errors.rs:OpaqueHiddenTypeDiag` resolves on the SemOS port path.
infer_opaque_type = opaque type whose hidden type is being assigned
infer_hidden_type = opaque type being used as hidden type
