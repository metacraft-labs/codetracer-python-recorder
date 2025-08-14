"""Smoke test for codetracer_python_recorder wheel."""

def main() -> None:
    import codetracer_python_recorder as m
    print(m.hello())

if __name__ == "__main__":
    main()
