//! The shared [`Descriptor`] builder for the Python-sidecar adapters. Each
//! adapter differs only in its runtime id, bundle name, and status strings; this
//! collapses the identical prepare-environment / make-spec closure boilerplate
//! into one call.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::records::{ModelRecord, RuntimeId};

use crate::environment::EnvironmentManager;
use crate::python_runtime::Descriptor;
use crate::sidecar::{RuntimeBundle, SidecarError, bundle_spec};

/// How long to wait for a cooperatively-cancelled sidecar to acknowledge before
/// killing it — the default the MLX/embeddings adapters use.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// A [`Descriptor`] for a Python sidecar: it prepares the environment from the
/// bundle's lockfile and builds the launch spec, both locating the bundle
/// `bundle_name` under `search_roots`. `cooperative_cancel` controls whether a
/// cancelled stream keeps the sidecar warm (streaming models) or hard-kills it
/// (short one-shot models like embeddings).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sidecar_descriptor(
    runtime_id: RuntimeId,
    bundle_name: &'static str,
    preparing_status: &str,
    starting_status: &str,
    warm_window: Option<Duration>,
    cooperative_cancel: bool,
    environments: EnvironmentManager,
    search_roots: Arc<Vec<PathBuf>>,
    workdir_root: PathBuf,
) -> Descriptor {
    let prepare_id = runtime_id.clone();
    let prepare_roots = Arc::clone(&search_roots);
    let spec_id = runtime_id.clone();
    Descriptor {
        runtime_id: runtime_id.as_str().to_owned(),
        preparing_status: preparing_status.to_owned(),
        starting_status: starting_status.to_owned(),
        warm_window,
        prepare_environment: Arc::new(move |status| {
            let environments = environments.clone();
            let roots = Arc::clone(&prepare_roots);
            let id = prepare_id.clone();
            Box::pin(async move {
                let bundle = RuntimeBundle::require(bundle_name, &roots, &id)?;
                let env_dir = environments
                    .prepare(id.as_str(), &bundle.join("requirements.lock"), status)
                    .await
                    .map_err(|error| SidecarError::RuntimeFailed(error.to_string()))?;
                Ok(Some(env_dir))
            })
        }),
        make_spec: Arc::new(move |record: &ModelRecord, env_dir: Option<&Path>| {
            let bundle = RuntimeBundle::require(bundle_name, &search_roots, &spec_id)?;
            bundle_spec(
                &spec_id,
                record,
                &bundle,
                env_dir,
                &workdir_root,
                bundle_name,
                &[],
                cooperative_cancel,
                CANCEL_GRACE,
            )
        }),
    }
}
