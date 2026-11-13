//! Optional import progress without logging or process-global state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigImportPhase {
    Container,
    Schema,
    Decode,
    Extract,
    ResolveStyles,
    BuildTree,
    Prepared,
    ConvertPage,
    ReleaseSource,
    ResolveImages,
    Complete,
}

impl FigImportPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Schema => "schema",
            Self::Decode => "decode",
            Self::Extract => "extract",
            Self::ResolveStyles => "style-resolution",
            Self::BuildTree => "tree-build",
            Self::Prepared => "prepared",
            Self::ConvertPage => "page-conversion",
            Self::ReleaseSource => "source-release",
            Self::ResolveImages => "image-resolution",
            Self::Complete => "complete",
        }
    }
}
