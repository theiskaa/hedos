//! Community runtime manifests: the `manifest.toml` that describes a
//! user-installed runtime — what it can serve, how it launches (a `[serve]`
//! sidecar or a one-shot `[invoke]` command, optionally inside a `[vm]`), and
//! its permissions. Parsing uses the `toml` crate; the validation is the meat.

mod manifest;
mod provenance;

pub use manifest::{
    ManifestDetect, ManifestEnv, ManifestInvoke, ManifestPermissions, ManifestServe,
    ManifestValidationError, ManifestVm, RuntimeManifest,
};
pub use provenance::RuntimeProvenance;
