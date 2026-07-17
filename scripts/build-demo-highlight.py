"""Build the small, silent GitHub preview from verified demo screenshots."""

from __future__ import annotations

import argparse
from pathlib import Path

try:
    from PIL import Image
except ImportError as error:  # pragma: no cover - operator guidance
    raise SystemExit("Pillow is required: python -m pip install Pillow") from error


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--duration-ms", type=int, default=2200)
    parser.add_argument("--width", type=int, default=960)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    frames: list[Image.Image] = []

    for index in range(1, 9):
        matches = list(args.assets.glob(f"demo-{index:02d}-*.png"))
        if len(matches) != 1:
            raise SystemExit(
                f"Expected one demo frame for step {index}; found {len(matches)}."
            )
        source = Image.open(matches[0]).convert("RGB")
        height = round(source.height * args.width / source.width)
        resized = source.resize((args.width, height), Image.Resampling.LANCZOS)
        frames.append(
            resized.quantize(
                colors=128,
                method=Image.Quantize.MEDIANCUT,
                dither=Image.Dither.NONE,
            )
        )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(
        args.output,
        save_all=True,
        append_images=frames[1:],
        duration=args.duration_ms,
        loop=0,
        optimize=True,
        disposal=2,
    )
    print(
        f"Created {args.output} from {len(frames)} real-network frames "
        f"({args.output.stat().st_size} bytes)."
    )


if __name__ == "__main__":
    main()
