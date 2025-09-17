"""Example: module-level side effects vs main guard.

When imported, this module sets a constant; when run as __main__, it prints.
"""

SIDE_EFFECT = "loaded"  # module-level assignment (side effect)


def main() -> None:
    print("ok", SIDE_EFFECT)


if __name__ == "__main__":
    main()

