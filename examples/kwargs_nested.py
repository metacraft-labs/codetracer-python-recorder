"""Example: nested kwargs structure for structured encoding validation."""


def accept(**kwargs):  # noqa: D401 - simple demo
    """Return kwargs verbatim to keep behavior observable and stable."""
    return kwargs


def main() -> None:
    res = accept(a=1, b={"x": [1, 2], "y": {"z": 3}}, c=(4, 5))
    # Print a stable projection of nested structure
    print("ok", res["a"], sorted(res["b"].keys()), res["c"][0])


if __name__ == "__main__":
    main()

