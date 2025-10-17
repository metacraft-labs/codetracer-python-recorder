mod agents 

default:
    @just --list
    
# Development helpers for the monorepo

# Python version used for development
PYTHON_DEFAULT_VERSION := "3.13"

# Python versions used for multi-version testing/building with uv
PY_VERSIONS := "3.10 3.11 3.12 3.13"
PY_SHORT_VERSIONS := "10 11 12 13"

# Print toolchain versions to verify the dev environment
env:
    uv --version
    python3 --version
    cargo --version
    rustc --version
    maturin --version

clean:
    rm -rf .venv **/__pycache__ **/*.pyc **/*.pyo **/.pytest_cache
    rm -rf codetracer-python-recorder/target codetracer-python-recorder/**/*.so


# Create a clean local virtualenv for Python tooling (without editable packages installed)
venv version=PYTHON_DEFAULT_VERSION:
    uv sync -p {{version}}

# Build the module in dev mode
dev:
    uv run --directory codetracer-python-recorder maturin develop --uv --features integration-test

# Run unit tests of dev build
test: cargo-test py-test

# Run Rust unit tests without default features to link Python C library
cargo-test:
    uv run cargo nextest run --manifest-path codetracer-python-recorder/Cargo.toml --workspace --no-default-features

bench:
    just venv
    ROOT="$(pwd)"; \
    PYTHON_BIN="$ROOT/.venv/bin/python"; \
    if [ ! -x "$PYTHON_BIN" ]; then \
        PYTHON_BIN="$ROOT/.venv/Scripts/python.exe"; \
    fi; \
    if [ ! -x "$PYTHON_BIN" ]; then \
        echo "Python interpreter not found. Run 'just venv <version>' first."; \
        exit 1; \
    fi; \
    PERF_DIR="$ROOT/codetracer-python-recorder/target/perf"; \
    mkdir -p "$PERF_DIR"; \
    PYO3_PYTHON="$PYTHON_BIN" uv run cargo bench --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features --bench trace_filter && \
    CODETRACER_TRACE_FILTER_PERF=1 \
    CODETRACER_TRACE_FILTER_PERF_OUTPUT="$PERF_DIR/trace_filter_py.json" \
    uv run --group dev --group test pytest codetracer-python-recorder/tests/python/perf/test_trace_filter_perf.py -q

py-test:
    uv run --group dev --group test pytest codetracer-python-recorder/tests/python codetracer-pure-python-recorder

lint: lint-rust lint-errors

lint-rust:
    uv run cargo clippy --manifest-path codetracer-python-recorder/Cargo.toml --workspace --no-default-features -- -D clippy::panic

lint-errors:
    uv run python3 codetracer-python-recorder/scripts/lint_no_unwraps.py
    
# Run tests only on the pure recorder
test-pure:
    uv run --group dev --group test pytest codetracer-pure-python-recorder

# Inspect ad-hoc error handling patterns across the Rust/Python recorder
errors-audit:
    @echo "== PyRuntimeError construction =="
    @rg --color=never --no-heading -n "PyRuntimeError::new_err" codetracer-python-recorder/src codetracer-python-recorder/tests codetracer-python-recorder/codetracer_python_recorder || true
    @echo
    @echo "== unwrap()/expect()/panic! usage =="
    @rg --color=never --no-heading -n "\\.unwrap\\(" codetracer-python-recorder/src || true
    @rg --color=never --no-heading -n "\\.expect\\(" codetracer-python-recorder/src || true
    @rg --color=never --no-heading -n "panic!" codetracer-python-recorder/src || true
    @echo
    @echo "== Python-side bare RuntimeError/ValueError =="
    @rg --color=never --no-heading -n "raise RuntimeError" codetracer-python-recorder/codetracer_python_recorder || true
    @rg --color=never --no-heading -n "raise ValueError" codetracer-python-recorder/codetracer_python_recorder || true

# Generate combined coverage artefacts for both crates
coverage:
    just coverage-rust
    just coverage-python

coverage-rust:
    mkdir -p codetracer-python-recorder/target/coverage/rust
    LLVM_COV="$(command -v llvm-cov)" LLVM_PROFDATA="$(command -v llvm-profdata)" \
        uv run cargo llvm-cov nextest --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features --lcov --output-path codetracer-python-recorder/target/coverage/rust/lcov.info
    LLVM_COV="$(command -v llvm-cov)" LLVM_PROFDATA="$(command -v llvm-profdata)" \
        uv run cargo llvm-cov report --summary-only --json --manifest-path codetracer-python-recorder/Cargo.toml --output-path codetracer-python-recorder/target/coverage/rust/summary.json
    python3 codetracer-python-recorder/scripts/render_rust_coverage_summary.py \
        codetracer-python-recorder/target/coverage/rust/summary.json --root "$(pwd)"

coverage-python:
    mkdir -p codetracer-python-recorder/target/coverage/python
    uv run --group dev --group test pytest --cov=codetracer_python_recorder --cov-report=term --cov-report=xml:codetracer-python-recorder/target/coverage/python/coverage.xml --cov-report=json:codetracer-python-recorder/target/coverage/python/coverage.json codetracer-python-recorder/tests/python

# Build the module in release mode
build:
    just venv
    uv run --directory codetracer-python-recorder maturin build --release --sdist

# Build wheels for all target Python versions with maturin
build-all:
    just venv
    uv run --directory codetracer-python-recorder maturin build --release --sdist --interpreter {{PY_VERSIONS}}

# Smoke the built Rust wheels across versions using uv
test-all:
    for v in {{PY_SHORT_VERSIONS}}; do \
        file=(codetracer-python-recorder/target/wheels/codetracer_python_recorder-*-cp3$v-cp3$v-*.whl); \
        file="${file[0]}"; \
        uv run -p "python3.$v" --with "${file}" --with pytest -- pytest -q; \
    done

# Install a freshly built artifact and run a CLI smoke test
smoke-wheel artifact="wheel" interpreter=".venv/bin/python":
    just build
    VENV_DIR="$(mktemp -d)"; \
    trap 'rm -rf "$VENV_DIR"' EXIT; \
    "{{interpreter}}" -m venv "$VENV_DIR"; \
    VENV_PY="$VENV_DIR/bin/python"; \
    if [ ! -x "$VENV_PY" ]; then \
        VENV_PY="$VENV_DIR/Scripts/python.exe"; \
    fi; \
    "$VENV_PY" -m pip install --upgrade pip; \
    FILE="$("$VENV_PY" scripts/select_recorder_artifact.py --wheel-dir codetracer-python-recorder/target/wheels --mode "{{artifact}}")"; \
    "$VENV_PY" -m pip install "$FILE"; \
    "$VENV_PY" -m codetracer_python_recorder --help >/dev/null
