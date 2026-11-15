//! Preserve the existing Core adapter name over the schema-shared serializer.
pub(super) type ReplaceChildren<'a, S> =
    jian_ops_schema::serde_children::ReplaceChildren<S, super::SharedNodes<'a>>;
