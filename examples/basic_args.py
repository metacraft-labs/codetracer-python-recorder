"""Example: function with all Python argument kinds.

Covers positional-only, positional-or-keyword, keyword-only, *args, **kwargs.
"""

def f(p, /, q, *args, r, **kwargs):  # noqa: D401 - simple demo
    """Return a tuple to keep behavior deterministic."""
    return (p, q, args, r, kwargs)


def main() -> None:
    res = f(1, 2, 3, 4, 5, r=6, a=7, b=8)
    # Minimal stable output
    print("ok", res[0], res[1], len(res[2]), res[3], sorted(res[4].items()))


if __name__ == "__main__":
    main()

