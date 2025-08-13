import codetracer_python_recorder as m


def test_hello_returns_expected_string() -> None:
    assert m.hello() == "Hello from codetracer-python-recorder (Rust)"
