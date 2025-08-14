default:
    @just --list
    
# Development helpers for the monorepo

# Python version used for development
PYTHON_DEV_VERSION := "3.13"

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


# Create a local virtualenv for Python tooling
venv:
    uv venv -p python{{PYTHON_DEV_VERSION}}

dev:
    just venv
    uv run --directory codetracer-python-recorder maturin develop --uv

test:
    just venv
    uv run pytest

build:
    uv run --directory codetracer-python-recorder maturin build

# Run the test suite across multiple Python versions using uv
test-uv-all:
    uv python install {{PY_VERSIONS}}
    for v in {{PY_VERSIONS}}; do uv run -p "$v" -m unittest discover -v; done

# Build wheels for all target Python versions with maturin
build-rust-uv-all:
    for v in {{PY_VERSIONS}}; do \
        uv run -p "$v" --directory codetracer-python-recorder maturin build --release; \
    done

# Smoke the built Rust wheels across versions using uv
test-rust-uv-all:
    for v in {{PY_SHORT_VERSIONS}}; do \
        file=(codetracer-python-recorder/target/wheels/codetracer_python_recorder-*-cp3$v-cp3$v-*.whl); \
        file="${file[0]}"; \
        uv run -p "python3.$v" --with "${file}" --with pytest -- pytest -q; \
    done
