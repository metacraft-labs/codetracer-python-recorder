"""Example: raising and handling an exception.

Print the exception inside the except block to exercise printing under tracing.
"""


def boom() -> None:
    raise ValueError("boom!")


def main() -> None:
    try:
        boom()
    except Exception as e:  # noqa: BLE001 - intentional for example
        # Printing inside except triggers additional Python activity
        print("handled", type(e).__name__)


if __name__ == "__main__":
    main()

