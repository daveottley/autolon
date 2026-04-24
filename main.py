from bot.logic import run

def main() -> int:
    print("AutoLon starting...")

    try:
        run()
    except Exception as exc:
        print(f"Error: {exc}")
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

