use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use codetracer_python_recorder::trace_filter::config::TraceFilterConfig;
use codetracer_python_recorder::trace_filter::engine::{TraceFilterEngine, ValueKind};
use codetracer_python_recorder::CodeObjectWrapper;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyCode, PyModule};
use tempfile::{tempdir, TempDir};

const CALLS_PER_BATCH: usize = 10_000;
const LOCALS_PER_CALL: usize = 50;
const FUNCTIONS_PER_MODULE: usize = 10;
const SERVICES_MODULES: usize = 6;
const WORKER_MODULES: usize = 3;
const EXTERNAL_MODULES: usize = 1;
const UNIQUE_CODE_OBJECTS: usize =
    (SERVICES_MODULES + WORKER_MODULES + EXTERNAL_MODULES) * FUNCTIONS_PER_MODULE;

fn bench_trace_filters(c: &mut Criterion) {
    pyo3::prepare_freethreaded_python();

    let workspace = BenchWorkspace::initialise();
    let dataset = Arc::clone(&workspace.dataset);
    let scenarios = workspace.build_scenarios();

    let mut group = c.benchmark_group("trace_filter");
    group.throughput(Throughput::Elements(CALLS_PER_BATCH as u64));

    for scenario in scenarios {
        let engine = Arc::clone(&scenario.engine);
        prewarm_engine(engine.as_ref(), dataset.as_ref());

        let dataset_ref = Arc::clone(&dataset);
        group.bench_function(BenchmarkId::new("workload", scenario.label), move |b| {
            b.iter(|| run_workload(engine.as_ref(), dataset_ref.as_ref()));
        });
    }

    group.finish();
}

criterion_group!(trace_filter, bench_trace_filters);
criterion_main!(trace_filter);

fn run_workload(engine: &TraceFilterEngine, dataset: &WorkloadDataset) {
    let kind = ValueKind::Local;
    Python::with_gil(|py| {
        for &index in &dataset.event_indices {
            let code = dataset.codes[index].as_ref();
            let resolution = engine
                .resolve(py, code, None)
                .expect("trace filter resolution should succeed during benchmarking");
            let policy = resolution.value_policy();
            for name in dataset.locals.iter() {
                black_box(policy.decide(kind, name));
            }
        }
    });
}

fn prewarm_engine(engine: &TraceFilterEngine, dataset: &WorkloadDataset) {
    Python::with_gil(|py| {
        for code in &dataset.codes {
            let _ = engine
                .resolve(py, code.as_ref(), None)
                .expect("prewarm resolution failed");
        }
    });
}

struct BenchWorkspace {
    _root: TempDir,
    filters: FilterFiles,
    dataset: Arc<WorkloadDataset>,
}

impl BenchWorkspace {
    fn initialise() -> Self {
        let root = tempdir().expect("failed to create benchmark workspace");
        let project_root = root.path().to_path_buf();
        let codetracer_dir = project_root.join(".codetracer");
        fs::create_dir_all(&codetracer_dir).expect("failed to create .codetracer directory");

        let filters = FilterFiles::create(&codetracer_dir);
        let dataset = Python::with_gil(|py| WorkloadDataset::new(py, &project_root))
            .expect("failed to build workload dataset");

        BenchWorkspace {
            _root: root,
            filters,
            dataset: Arc::new(dataset),
        }
    }

    fn build_scenarios(&self) -> Vec<FilterScenario> {
        vec![
            FilterScenario::new("baseline", &self.filters.baseline),
            FilterScenario::new("glob", &self.filters.glob),
            FilterScenario::new("regex", &self.filters.regex),
        ]
    }
}

struct FilterScenario {
    label: &'static str,
    engine: Arc<TraceFilterEngine>,
}

impl FilterScenario {
    fn new(label: &'static str, path: &Path) -> Self {
        let config = TraceFilterConfig::from_paths(&[path.to_path_buf()])
            .expect("failed to load benchmark trace filter");
        let engine = TraceFilterEngine::new(config);
        FilterScenario {
            label,
            engine: Arc::new(engine),
        }
    }
}

struct FilterFiles {
    baseline: PathBuf,
    glob: PathBuf,
    regex: PathBuf,
}

impl FilterFiles {
    fn create(dir: &Path) -> Self {
        let baseline = dir.join("bench-baseline.toml");
        let glob = dir.join("bench-glob.toml");
        let regex = dir.join("bench-regex.toml");

        fs::write(&baseline, baseline_config()).expect("failed to write baseline filter");
        fs::write(&glob, glob_config()).expect("failed to write glob filter");
        fs::write(&regex, regex_config()).expect("failed to write regex filter");

        FilterFiles {
            baseline,
            glob,
            regex,
        }
    }
}

struct WorkloadDataset {
    codes: Vec<Arc<CodeObjectWrapper>>,
    event_indices: Vec<usize>,
    locals: Arc<[String]>,
}

impl WorkloadDataset {
    fn new(py: Python<'_>, project_root: &Path) -> PyResult<Self> {
        let local_names = build_local_names();
        let specs = build_module_specs();
        let mut codes = Vec::with_capacity(UNIQUE_CODE_OBJECTS);

        for spec in specs {
            let file_path = project_root.join(&spec.relative_path);
            let source = module_source(&spec.func_prefix, spec.functions, &local_names);

            let source_c = CString::new(source).expect("module source cannot contain NUL bytes");
            let file_c = CString::new(file_path.to_string_lossy().into_owned())
                .expect("file path cannot contain NUL bytes");
            let module_c = CString::new(spec.module_name.clone())
                .expect("module name cannot contain NUL bytes");

            let module = PyModule::from_code(
                py,
                source_c.as_c_str(),
                file_c.as_c_str(),
                module_c.as_c_str(),
            )?;
            for idx in 0..spec.functions {
                let func_name = format!("{}_{}", spec.func_prefix, idx);
                let func: Bound<'_, PyAny> = module.getattr(&func_name)?;
                let code = func.getattr("__code__")?.downcast_into::<PyCode>()?;
                codes.push(Arc::new(CodeObjectWrapper::new(py, &code)));
            }
        }

        assert_eq!(
            codes.len(),
            UNIQUE_CODE_OBJECTS,
            "unexpected number of code objects generated for benchmark"
        );

        let mut event_indices = Vec::with_capacity(CALLS_PER_BATCH);
        for i in 0..CALLS_PER_BATCH {
            event_indices.push(i % codes.len());
        }

        let locals: Arc<[String]> = Arc::from(local_names);

        Ok(WorkloadDataset {
            codes,
            event_indices,
            locals,
        })
    }
}

struct ModuleSpec {
    relative_path: String,
    module_name: String,
    func_prefix: String,
    functions: usize,
}

impl ModuleSpec {
    fn new(
        relative_path: String,
        module_name: String,
        func_prefix: String,
        functions: usize,
    ) -> Self {
        ModuleSpec {
            relative_path,
            module_name,
            func_prefix,
            functions,
        }
    }
}

fn build_module_specs() -> Vec<ModuleSpec> {
    let mut specs = Vec::with_capacity(SERVICES_MODULES + WORKER_MODULES + EXTERNAL_MODULES);

    for idx in 0..SERVICES_MODULES {
        specs.push(ModuleSpec::new(
            format!("bench_pkg/services/api/module_{idx}.py"),
            format!("bench_pkg.services.api.module_{idx}"),
            format!("api_handler_{idx}"),
            FUNCTIONS_PER_MODULE,
        ));
    }

    for idx in 0..WORKER_MODULES {
        specs.push(ModuleSpec::new(
            format!("bench_pkg/jobs/worker/module_{idx}.py"),
            format!("bench_pkg.jobs.worker.module_{idx}"),
            format!("worker_task_{idx}"),
            FUNCTIONS_PER_MODULE,
        ));
    }

    for idx in 0..EXTERNAL_MODULES {
        specs.push(ModuleSpec::new(
            format!("bench_pkg/external/integration_{idx}.py"),
            format!("bench_pkg.external.integration_{idx}"),
            format!("integration_op_{idx}"),
            FUNCTIONS_PER_MODULE,
        ));
    }

    specs
}

fn module_source(func_prefix: &str, function_count: usize, local_names: &[String]) -> String {
    let mut source = String::new();
    for idx in 0..function_count {
        let func_name = format!("{func_prefix}_{idx}");
        source.push_str("def ");
        source.push_str(&func_name);
        source.push_str("(value):\n");
        for (offset, name) in local_names.iter().enumerate() {
            source.push_str("    ");
            source.push_str(name);
            source.push_str(" = value + ");
            source.push_str(&offset.to_string());
            source.push('\n');
        }
        source.push_str("    return value\n\n");
    }
    source
}

fn build_local_names() -> Vec<String> {
    let mut names = Vec::with_capacity(LOCALS_PER_CALL);
    for idx in 0..15 {
        names.push(format!("public_field_{idx}"));
    }
    for idx in 0..15 {
        names.push(format!("secret_field_{idx}"));
    }
    for idx in 0..10 {
        names.push(format!("token_{idx}"));
    }
    names.push("password_hash".to_string());
    names.push("api_key".to_string());
    names.push("credit_card".to_string());
    names.push("session_id".to_string());
    names.push("metric_latency".to_string());
    names.push("metric_throughput".to_string());
    names.push("metric_error_rate".to_string());
    names.push("masked_value".to_string());
    names.push("debug_flag".to_string());
    names.push("trace_id".to_string());

    assert_eq!(names.len(), LOCALS_PER_CALL, "local name count mismatch");
    names
}

fn baseline_config() -> String {
    r#"
[meta]
name = "bench-baseline"
version = 1
description = "Tracing baseline without additional filter overhead."

[scope]
default_exec = "trace"
default_value_action = "allow"
"#
    .trim_start_matches('\n')
    .to_string()
}

fn glob_config() -> String {
    r#"
[meta]
name = "bench-glob"
version = 1
description = "Glob-heavy rule set for microbenchmark coverage."

[scope]
default_exec = "trace"
default_value_action = "allow"

[[scope.rules]]
selector = "pkg:bench_pkg.services.api.*"
value_default = "redact"
reason = "Redact service locals except approved public fields"
[[scope.rules.value_patterns]]
selector = "local:glob:public_*"
action = "allow"
[[scope.rules.value_patterns]]
selector = "local:glob:metric_*"
action = "allow"
[[scope.rules.value_patterns]]
selector = "local:glob:secret_*"
action = "redact"
[[scope.rules.value_patterns]]
selector = "local:glob:token_*"
action = "redact"
[[scope.rules.value_patterns]]
selector = "local:glob:masked_*"
action = "allow"
[[scope.rules.value_patterns]]
selector = "local:glob:password_*"
action = "redact"

[[scope.rules]]
selector = "file:glob:bench_pkg/jobs/worker/module_*.py"
exec = "skip"
reason = "Disable redundant worker instrumentation"

[[scope.rules]]
selector = "pkg:bench_pkg.external.integration_*"
value_default = "redact"
[[scope.rules.value_patterns]]
selector = "local:glob:metric_*"
action = "allow"
[[scope.rules.value_patterns]]
selector = "local:glob:public_*"
action = "allow"
"#
    .trim_start_matches('\n')
    .to_string()
}

fn regex_config() -> String {
    r#"
[meta]
name = "bench-regex"
version = 1
description = "Regex-heavy rule set for microbenchmark coverage."

[scope]
default_exec = "trace"
default_value_action = "allow"

[[scope.rules]]
selector = 'pkg:regex:^bench_pkg\.services\.api\.module_\d+$'
value_default = "redact"
reason = "Regex match on service modules"
[[scope.rules.value_patterns]]
selector = 'local:regex:^(public|metric)_\w+$'
action = "allow"
[[scope.rules.value_patterns]]
selector = 'local:regex:^(secret|token)_\w+$'
action = "redact"
[[scope.rules.value_patterns]]
selector = 'local:regex:^(password|api|credit|session)_.*$'
action = "redact"

[[scope.rules]]
selector = 'file:regex:^bench_pkg/jobs/worker/module_\d+\.py$'
exec = "skip"
reason = "Regex skip for worker modules"

[[scope.rules]]
selector = 'obj:regex:^bench_pkg\.external\.integration_\d+\.integration_op_\d+$'
value_default = "redact"
[[scope.rules.value_patterns]]
selector = 'local:regex:^masked_.*$'
action = "allow"
[[scope.rules.value_patterns]]
selector = 'local:regex:^metric_.*$'
action = "allow"
"#
    .trim_start_matches('\n')
    .to_string()
}
