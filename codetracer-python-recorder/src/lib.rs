//! Runtime tracing module backed by PyO3.
//!
//! Tracer implementations must return `CallbackResult` from every callback so they can
//! signal when CPython should disable further monitoring for a location by propagating
//! the `sys.monitoring.DISABLE` sentinel.

pub mod code_object;
mod errors;
mod ffi;
mod logging;
mod module_identity;
pub mod monitoring;
mod policy;
mod runtime;
mod session;
pub mod trace_filter;

pub use crate::code_object::{CodeObjectRegistry, CodeObjectWrapper};
pub use crate::monitoring as tracer;
pub use crate::monitoring::{
    flush_installed_tracer, install_tracer, uninstall_tracer, CallbackOutcome, CallbackResult,
    EventSet, Tracer,
};
pub use crate::session::{flush_tracing, is_tracing, start_tracing, stop_tracing};

use pyo3::prelude::*;

#[pyfunction]
fn managed_upload_materialized_trace(trace_dir: String) -> PyResult<String> {
    let upload =
        codetracer_ctfs::trace_storage::upload_materialized_artifacts_from_env(trace_dir, "python")
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.message))?;
    Ok(upload
        .uploads
        .first()
        .map(|upload| upload.receipt.object_key.clone())
        .unwrap_or_default())
}

#[cfg(test)]
mod shared_trace_storage_adapter_tests {
    use codetracer_ctfs::trace_storage::{
        ManagedTraceSender, ManagedUploadKind, ManagedUploadObject, ManagedUploadReceipt,
        SenderError, SenderHealth, SharedSenderBackend, StorageMode, TraceStorageConfig,
        TRACE_STORAGE_SCHEMA,
    };

    #[test]
    fn python_recorder_binds_shared_trace_storage_config() {
        let config = TraceStorageConfig::from_json(include_str!(
            "../../../codetracer-trace-format/codetracer_ctfs/tests/fixtures/trace_storage/storage_config.full.json"
        ))
        .expect("shared trace-storage fixture parses through codetracer_ctfs");

        assert_eq!(config.schema, TRACE_STORAGE_SCHEMA);
        assert_eq!(config.materialized_artifact_policy.pool, "artifacts-hot");
        assert!(config.split_policy.enabled);
    }

    #[test]
    fn python_recorder_binds_shared_managed_upload_storage_config() {
        let config = TraceStorageConfig::from_json(include_str!(
            "../../../codetracer-trace-format/codetracer_ctfs/tests/fixtures/trace_storage/storage_config.managed_upload.json"
        ))
        .expect("shared managed-upload trace-storage fixture parses through codetracer_ctfs");

        assert_eq!(config.schema, TRACE_STORAGE_SCHEMA);
        match config.mode {
            StorageMode::ManagedUpload { control_plane_url } => {
                assert_eq!(control_plane_url, "https://ci.example.test/managed-upload");
            }
            StorageMode::DirectStorage { .. } => panic!("expected managed upload storage mode"),
        }
        assert_eq!(config.storage_servers[1].credential_ref.provider, "vault");
        assert_eq!(config.replication.target_replicas, 3);
    }

    #[derive(Default)]
    struct PythonBindingBackend {
        uploaded: Vec<String>,
    }

    impl SharedSenderBackend for PythonBindingBackend {
        fn upload_slice(
            &mut self,
            object: &ManagedUploadObject,
        ) -> Result<ManagedUploadReceipt, SenderError> {
            self.upload(object)
        }

        fn upload_materialized_artifact(
            &mut self,
            object: &ManagedUploadObject,
        ) -> Result<ManagedUploadReceipt, SenderError> {
            self.upload(object)
        }

        fn upload_manifest(
            &mut self,
            object: &ManagedUploadObject,
        ) -> Result<ManagedUploadReceipt, SenderError> {
            self.upload(object)
        }

        fn finalize(
            &mut self,
            _request: &codetracer_ctfs::trace_storage::ManagedFinalizeRequest,
        ) -> Result<(), SenderError> {
            Ok(())
        }

        fn health(&self) -> SenderHealth {
            SenderHealth {
                healthy: true,
                message: "python binding backend".to_string(),
            }
        }
    }

    impl PythonBindingBackend {
        fn upload(
            &mut self,
            object: &ManagedUploadObject,
        ) -> Result<ManagedUploadReceipt, SenderError> {
            self.uploaded.push(object.object_key.clone());
            Ok(ManagedUploadReceipt {
                object_key: object.object_key.clone(),
                storage_pool_id: "shared-local".to_string(),
                storage_server_id: "local-storage-1".to_string(),
                storage_endpoint_uri: "local://codetracer-ci/storage-service".to_string(),
            })
        }
    }

    #[test]
    fn python_recorder_uses_shared_managed_sender_for_materialized_artifacts() {
        let mut sender =
            ManagedTraceSender::new(PythonBindingBackend::default(), "python-finalize");
        sender
            .upload_materialized_artifact(ManagedUploadObject {
                object_key: "traces/tenant/python/materialized-trace-v1.json".to_string(),
                local_path: "/tmp/python/materialized-trace-v1.json".to_string(),
                content_length: 256,
                sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                kind: ManagedUploadKind::MaterializedArtifact {
                    artifact_kind: "materialized_trace_v1".to_string(),
                },
            })
            .expect("shared managed sender accepts materialized artifact");
        assert_eq!(sender.backend().uploaded.len(), 1);
    }
}

/// Python module definition.
#[pymodule]
fn codetracer_python_recorder(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize logging on import so users see logs without extra setup.
    // Respect RUST_LOG if present; otherwise keep our crate quiet unless warnings/errors occur.
    logging::init_rust_logging_with_default("codetracer_python_recorder=warn");
    ffi::register_exceptions(m)?;
    m.add_function(wrap_pyfunction!(start_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(stop_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(is_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(flush_tracing, m)?)?;
    m.add_function(wrap_pyfunction!(policy::configure_policy_py, m)?)?;
    m.add_function(wrap_pyfunction!(policy::py_configure_policy_from_env, m)?)?;
    m.add_function(wrap_pyfunction!(policy::py_policy_snapshot, m)?)?;
    m.add_function(wrap_pyfunction!(managed_upload_materialized_trace, m)?)?;
    Ok(())
}
